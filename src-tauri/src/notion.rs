// lib/notion.js 의 reqwest 이식. Notion API 는 CORS 미허용이라 Rust 에서 호출한다.
use serde_json::{json, Value};

const API: &str = "https://api.notion.com/v1";
const VERSION: &str = "2022-06-28";

#[derive(Clone, Debug)]
pub struct Schema {
    pub title_prop: String,
    pub date_prop: String,
    pub done_prop: Option<String>,
    pub person_prop: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NotionUser {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct NotionPage {
    pub id: String,
    pub title: String,
    pub due_at: Option<i64>,
    pub done: bool,
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn req(
    token: &str,
    method: reqwest::Method,
    api_path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{}{}", API, api_path);
    let mut rb = client()
        .request(method, &url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Notion-Version", VERSION)
        .header("Content-Type", "application/json");
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let res = rb.send().await.map_err(|e| e.to_string())?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(300).collect();
        return Err(format!("Notion API {}: {}", status.as_u16(), snippet));
    }
    res.json::<Value>().await.map_err(|e| e.to_string())
}

/// DB 스키마에서 제목/날짜/체크박스/담당자(사람) 속성을 자동 감지
pub async fn get_schema(token: &str, db_id: &str) -> Result<Schema, String> {
    let db = req(token, reqwest::Method::GET, &format!("/databases/{}", db_id), None).await?;
    let props = db
        .get("properties")
        .and_then(|p| p.as_object())
        .ok_or_else(|| "데이터베이스 속성을 읽을 수 없습니다".to_string())?;
    let mut title_prop: Option<String> = None;
    let mut date_prop: Option<String> = None;
    let mut done_prop: Option<String> = None;
    let mut person_prop: Option<String> = None;
    for (name, p) in props {
        let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "title" && title_prop.is_none() {
            title_prop = Some(name.clone());
        }
        if ty == "date" && date_prop.is_none() {
            date_prop = Some(name.clone());
        }
        if ty == "checkbox" && done_prop.is_none() {
            done_prop = Some(name.clone());
        }
        if ty == "people" && person_prop.is_none() {
            person_prop = Some(name.clone());
        }
    }
    let title_prop = title_prop.ok_or_else(|| "데이터베이스에 제목(title) 속성이 없습니다".to_string())?;
    let date_prop = date_prop.ok_or_else(|| "데이터베이스에 날짜(date) 속성이 없습니다".to_string())?;
    Ok(Schema {
        title_prop,
        date_prop,
        done_prop,
        person_prop,
    })
}

/// 워크스페이스 사용자 목록 (봇 제외, 사람만)
pub async fn list_users(token: &str) -> Result<Vec<NotionUser>, String> {
    let mut users = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let qs = match &cursor {
            Some(c) => format!("?start_cursor={}&page_size=100", c),
            None => "?page_size=100".to_string(),
        };
        let r = req(token, reqwest::Method::GET, &format!("/users{}", qs), None).await?;
        if let Some(results) = r.get("results").and_then(|v| v.as_array()) {
            for u in results {
                if u.get("type").and_then(|t| t.as_str()) == Some("person") {
                    let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = u
                        .get("name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("(이름 없음)")
                        .to_string();
                    users.push(NotionUser { id, name });
                }
            }
        }
        let has_more = r.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);
        cursor = if has_more {
            r.get("next_cursor").and_then(|v| v.as_str()).map(|s| s.to_string())
        } else {
            None
        };
        if cursor.is_none() {
            break;
        }
    }
    Ok(users)
}

/// 특정 날짜의 페이지 조회 (assignee_id 가 있으면 담당자 필터 추가)
pub async fn query_day(
    token: &str,
    db_id: &str,
    schema: &Schema,
    day_str: &str,
    next_day_str: &str,
    assignee_id: Option<&str>,
) -> Result<Vec<NotionPage>, String> {
    let mut conds = vec![
        json!({ "property": schema.date_prop, "date": { "on_or_after": day_str } }),
        json!({ "property": schema.date_prop, "date": { "before": next_day_str } }),
    ];
    if let (Some(aid), Some(person)) = (assignee_id, schema.person_prop.as_ref()) {
        conds.push(json!({ "property": person, "people": { "contains": aid } }));
    }
    let body = json!({ "filter": { "and": conds }, "page_size": 100 });
    let r = req(
        token,
        reqwest::Method::POST,
        &format!("/databases/{}/query", db_id),
        Some(body),
    )
    .await?;
    let mut out = Vec::new();
    if let Some(results) = r.get("results").and_then(|v| v.as_array()) {
        for pg in results {
            let props = pg.get("properties");
            let title = props
                .and_then(|p| p.get(&schema.title_prop))
                .and_then(|t| t.get("title"))
                .and_then(|a| a.as_array())
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|x| x.get("plain_text").and_then(|s| s.as_str()))
                        .collect::<String>()
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(제목 없음)".to_string());
            let date_val = props
                .and_then(|p| p.get(&schema.date_prop))
                .and_then(|d| d.get("date"));
            let start = date_val
                .and_then(|d| d.get("start"))
                .and_then(|s| s.as_str());
            let has_time = start.map(|s| s.len() > 10).unwrap_or(false);
            let due_at = if has_time {
                start.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
            } else {
                None
            };
            let done = match &schema.done_prop {
                Some(dp) => props
                    .and_then(|p| p.get(dp))
                    .and_then(|c| c.get("checkbox"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
                None => false,
            };
            let id = pg.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            out.push(NotionPage {
                id,
                title,
                due_at,
                done,
            });
        }
    }
    Ok(out)
}

pub async fn set_done(
    token: &str,
    page_id: &str,
    done_prop: &Option<String>,
    done: bool,
) -> Result<(), String> {
    let dp = match done_prop {
        Some(d) => d,
        None => return Ok(()),
    };
    let body = json!({ "properties": { dp: { "checkbox": done } } });
    req(
        token,
        reqwest::Method::PATCH,
        &format!("/pages/{}", page_id),
        Some(body),
    )
    .await?;
    Ok(())
}

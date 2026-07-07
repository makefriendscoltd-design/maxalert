// 포인트 / 레벨 / 뱃지 / 정렬 / 보고서 — main.js 순수 로직의 Rust 이식.
// 창/IPC 와 무관한 순수 함수만 두고 단위 테스트 대상으로 삼는다.
use std::cmp::Ordering;

use chrono::{Datelike, Local, TimeZone, Timelike};
use serde_json::{json, Value};

use crate::store::{BadgeOwned, Data, LedgerEntry, Todo};

pub const SIREN_LEAD_MS: i64 = 3 * 60 * 1000; // 일정 3분 전부터 사이렌

// ---------- 레벨 ----------
// (min, title, icon)
pub const LEVELS: &[(i64, &str, &str)] = &[
    (0, "산만한 금붕어", "🐠"),
    (100, "두리번 다람쥐", "🐿️"),
    (250, "갈팡질팡 고양이", "🐱"),
    (500, "정신차린 부엉이", "🦉"),
    (900, "계획하는 비버", "🦫"),
    (1400, "몰입하는 돌고래", "🐬"),
    (2000, "칼같은 여우", "🦊"),
    (2800, "강철 늑대", "🐺"),
    (3800, "시간의 독수리", "🦅"),
    (5000, "타임 마스터", "⏳"),
];

// ---------- 뱃지 ----------
// (id, name, icon, desc)
pub const BADGES: &[(&str, &str, &str, &str)] = &[
    ("first-done", "첫 걸음", "🌱", "첫 할 일 완료"),
    ("early-bird", "얼리버드", "🌅", "오전 9시 전에 할 일 완료"),
    ("perfect-day", "퍼펙트 데이", "💯", "하루의 할 일 전부 완료"),
    ("no-snooze", "정면돌파", "🛡️", "미루기 없이 하루 클리어"),
    ("streak-3", "작심삼일 극복", "🔥", "3일 연속 전체 완료"),
    ("streak-7", "일주일의 기적", "🌈", "7일 연속 전체 완료"),
    ("siren-slayer", "사이렌 슬레이어", "🚨", "사이렌이 울리는 중에 완료 5회"),
    ("centurion", "백전노장", "⚔️", "누적 100개 완료"),
];

// 포인트 상점: 포스트잇 테마 가격
pub fn theme_price(id: &str) -> Option<i64> {
    match id {
        "classic" => Some(0),
        "neon" => Some(150),
        "kraft" => Some(200),
        "midnight" => Some(300),
        _ => None,
    }
}

// ---------- 날짜 헬퍼 (로컬 타임존, main.js Date 와 동일) ----------
pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

fn local_of(ms: i64) -> chrono::DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Local::now)
}

pub fn date_str_of(ms: i64) -> String {
    let d = local_of(ms);
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

pub fn today_str() -> String {
    date_str_of(now_ms())
}

pub fn tomorrow_str() -> String {
    date_str_of(now_ms() + 86_400_000)
}

fn hour_of(ms: i64) -> u32 {
    local_of(ms).hour()
}

// ---------- 포인트 ----------
pub fn add_points(data: &mut Data, delta: i64, reason: &str) {
    data.points.total = (data.points.total + delta).max(0);
    data.points.ledger.insert(
        0,
        LedgerEntry {
            at: now_ms(),
            delta,
            reason: reason.to_string(),
        },
    );
    if data.points.ledger.len() > 200 {
        data.points.ledger.truncate(200);
    }
}

#[derive(Clone, Debug)]
pub struct LevelInfo {
    pub n: usize,
    pub title: String,
    pub icon: String,
    pub min: i64,
    pub next: Option<i64>,
}

pub fn level_info(total: i64) -> LevelInfo {
    let mut idx = 0usize;
    for (i, lvl) in LEVELS.iter().enumerate() {
        if total >= lvl.0 {
            idx = i;
        }
    }
    let cur = LEVELS[idx];
    let next = LEVELS.get(idx + 1).map(|l| l.0);
    LevelInfo {
        n: idx + 1,
        title: cur.1.to_string(),
        icon: cur.2.to_string(),
        min: cur.0,
        next,
    }
}

pub fn own_badge(data: &Data, id: &str) -> bool {
    data.badges.iter().any(|b| b.id == id)
}

struct BadgeCtx {
    total_done: i64,
    siren_saves: i64,
    all_done: bool,
    no_snooze_day: bool,
    early_done: bool,
    streak_count: i64,
}

fn badge_test(id: &str, c: &BadgeCtx) -> bool {
    match id {
        "first-done" => c.total_done >= 1,
        "early-bird" => c.early_done,
        "perfect-day" => c.all_done,
        "no-snooze" => c.no_snooze_day,
        "streak-3" => c.streak_count >= 3,
        "streak-7" => c.streak_count >= 7,
        "siren-slayer" => c.siren_saves >= 5,
        "centurion" => c.total_done >= 100,
        _ => false,
    }
}

pub fn check_badges(data: &mut Data) {
    let today = today_str();
    let todays: Vec<&Todo> = data.todos.iter().filter(|t| t.date == today).collect();
    let all_done = !todays.is_empty() && todays.iter().all(|t| t.done);
    let ctx = BadgeCtx {
        total_done: data.stats.total_done,
        siren_saves: data.stats.siren_saves,
        all_done,
        no_snooze_day: all_done && todays.iter().all(|t| !(t.postpones.unwrap_or(0) > 0)),
        early_done: todays
            .iter()
            .any(|t| t.done && t.done_at.map(|ms| hour_of(ms) < 9).unwrap_or(false)),
        streak_count: data.streak.count,
    };
    // 획득 대상 수집 후 반영 (data 이중 대여 회피)
    let mut newly: Vec<(&'static str, &'static str)> = Vec::new();
    for (id, name, _icon, _desc) in BADGES.iter() {
        if !own_badge(data, id) && badge_test(id, &ctx) {
            newly.push((id, name));
        }
    }
    for (id, name) in newly {
        data.badges.push(BadgeOwned {
            id: id.to_string(),
            at: now_ms(),
        });
        add_points(data, 25, &format!("🏅 뱃지 획득: {}", name));
    }
}

pub fn today_points(data: &Data) -> i64 {
    let today = today_str();
    data.points
        .ledger
        .iter()
        .filter(|e| date_str_of(e.at) == today)
        .map(|e| e.delta)
        .sum()
}

pub fn profile_payload(data: &Data) -> Value {
    let total = data.points.total;
    let li = level_info(total);
    let badges: Vec<Value> = BADGES
        .iter()
        .map(|(id, name, icon, desc)| {
            let at = data
                .badges
                .iter()
                .find(|b| &b.id == id)
                .map(|b| Value::from(b.at))
                .unwrap_or(Value::Null);
            json!({ "id": id, "name": name, "icon": icon, "desc": desc, "at": at })
        })
        .collect();
    let stats_v = serde_json::to_value(&data.stats).unwrap_or(Value::Null);
    let streak_v = serde_json::to_value(&data.streak).unwrap_or(Value::Null);
    let ledger_slice: Vec<&LedgerEntry> = data.points.ledger.iter().take(12).collect();
    let ledger_v = serde_json::to_value(&ledger_slice).unwrap_or(Value::Null);
    json!({
        "total": total,
        "todayPoints": today_points(data),
        "level": {
            "n": li.n,
            "title": li.title,
            "icon": li.icon,
            "min": li.min,
            "next": li.next,
        },
        "badges": badges,
        "stats": stats_v,
        "streak": streak_v,
        "ledger": ledger_v,
    })
}

// ---------- 정렬 ----------
pub fn sort_todos(a: &Todo, b: &Todo) -> Ordering {
    if a.done != b.done {
        return if a.done { Ordering::Greater } else { Ordering::Less };
    }
    match (a.due_at, b.due_at) {
        (None, None) => a.created_at.cmp(&b.created_at),
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

// ---------- 색상 (main.js pickColor 와 동일 해시) ----------
pub fn pick_color(seed: &str) -> String {
    const COLORS: [&str; 6] = ["yellow", "pink", "blue", "green", "purple", "orange"];
    let mut h: u32 = 0;
    // JS: for (const c of seed) h = (h*31 + c.charCodeAt(0)) >>> 0
    // for..of 는 코드포인트 단위, charCodeAt(0) 은 그 코드포인트의 첫 UTF-16 유닛.
    for cp in seed.chars() {
        let code = cp as u32;
        let unit = if code <= 0xFFFF {
            code
        } else {
            0xD800 + ((code - 0x10000) >> 10)
        };
        h = h.wrapping_mul(31).wrapping_add(unit);
    }
    COLORS[(h % 6) as usize].to_string()
}

// ---------- 사이렌 판정 ----------
pub fn siren_eligible(t: &Todo, now: i64) -> bool {
    !t.done
        && t.due_at.is_some()
        && (t.due_at.unwrap() - now) <= SIREN_LEAD_MS
        && t.ack_due != t.due_at
}

// ---------- 보상 ----------
#[derive(Clone, Debug)]
pub struct RewardInfo {
    pub streak: i64,
    pub today: i64,
    pub total: i64,
    pub title: String,
    pub icon: String,
    pub next: String,
    pub min: i64,
}

/// 오늘 할 일을 모두 완료했고 오늘 아직 보상하지 않았다면 상태를 갱신하고 보상 정보를 반환한다.
/// (창 열기·저장은 호출자 몫)
pub fn maybe_reward(data: &mut Data) -> Option<RewardInfo> {
    let today = today_str();
    let todays: Vec<&Todo> = data.todos.iter().filter(|t| t.date == today).collect();
    if todays.is_empty() || !todays.iter().all(|t| t.done) {
        return None;
    }
    if data.last_reward_date.as_deref() == Some(today.as_str()) {
        return None;
    }
    data.last_reward_date = Some(today.clone());
    let yesterday = date_str_of(now_ms() - 86_400_000);
    data.streak.count = if data.streak.last_date.as_deref() == Some(yesterday.as_str()) {
        data.streak.count + 1
    } else {
        1
    };
    data.streak.last_date = Some(today.clone());
    add_points(data, 50, "🎉 오늘의 할 일 전체 완료 보너스");
    check_badges(data);
    let li = level_info(data.points.total);
    Some(RewardInfo {
        streak: data.streak.count,
        today: today_points(data),
        total: data.points.total,
        title: li.title,
        icon: li.icon,
        next: li.next.map(|n| n.to_string()).unwrap_or_default(),
        min: li.min,
    })
}

// ---------- 일일 보고서 (main.js buildDailyReport 와 동일) ----------
pub fn build_daily_report(data: &Data) -> String {
    let today = today_str();
    let mut todays: Vec<Todo> = data.todos.iter().filter(|t| t.date == today).cloned().collect();
    todays.sort_by(sort_todos);
    let done: Vec<&Todo> = todays.iter().filter(|t| t.done).collect();
    let fmt = |ms: i64| -> String {
        let d = local_of(ms);
        format!("{:02}:{:02}", d.hour(), d.minute())
    };
    let line = |t: &Todo| -> String {
        let time = match t.due_at {
            Some(ms) => format!("`{}` ", fmt(ms)),
            None => String::new(),
        };
        format!("- {}{}", time, t.title)
    };
    let name = data.settings.notion_assignee.as_ref().map(|a| a.name.clone());
    let header = match name {
        Some(n) if !n.is_empty() => format!("{} | {} 업무 보고", n, today),
        _ => format!("{} 업무 보고", today),
    };
    let mut l: Vec<String> = Vec::new();
    l.push(format!("## 📋 {}", header));
    l.push(String::new());
    l.push(format!("완료 **{}** / 전체 {}", done.len(), todays.len()));
    l.push(String::new());
    if done.is_empty() {
        l.push("- (완료한 항목 없음)".to_string());
    } else {
        l.push(done.iter().map(|t| line(t)).collect::<Vec<_>>().join("\n"));
    }
    l.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Data, Todo};

    fn todo(id: &str, done: bool, due_at: Option<i64>, created_at: i64) -> Todo {
        Todo {
            id: id.to_string(),
            title: id.to_string(),
            color: "yellow".to_string(),
            date: today_str(),
            done,
            created_at,
            due_at,
            done_at: None,
            awarded: None,
            postpones: None,
            ack_due: None,
            notion_page_id: None,
            notion_due_at: None,
            notion_done: None,
        }
    }

    #[test]
    fn level_boundaries() {
        // 경계값에서 정확히 다음 레벨로 올라간다.
        assert_eq!(level_info(0).n, 1);
        assert_eq!(level_info(99).n, 1);
        assert_eq!(level_info(100).n, 2);
        assert_eq!(level_info(249).n, 2);
        assert_eq!(level_info(250).n, 3);
        // 최상위 레벨은 next 가 없다.
        let top = level_info(5000);
        assert_eq!(top.n, LEVELS.len());
        assert_eq!(top.title, "타임 마스터");
        assert_eq!(top.next, None);
        // 중간 레벨은 다음 경계를 가리킨다.
        assert_eq!(level_info(100).next, Some(250));
    }

    #[test]
    fn badges_awarded_once_and_by_condition() {
        let mut data = Data::default();
        // 완료 1개 → first-done, 완료 100개 누적 → centurion
        data.stats.total_done = 100;
        data.todos.push(todo("a", true, None, now_ms()));
        check_badges(&mut data);
        assert!(own_badge(&data, "first-done"));
        assert!(own_badge(&data, "centurion"));
        // 조건 미달 뱃지는 없음
        assert!(!own_badge(&data, "streak-7"));
        let badge_count = data.badges.len();
        let total_after_first = data.points.total;
        // 재실행해도 중복 지급/추가 없음
        check_badges(&mut data);
        assert_eq!(data.badges.len(), badge_count);
        assert_eq!(data.points.total, total_after_first);
    }

    #[test]
    fn sort_order_done_last_then_due_then_created() {
        let mut v = vec![
            todo("done1", true, Some(1000), 1),
            todo("notime", false, None, 5),
            todo("late", false, Some(3000), 2),
            todo("early", false, Some(1000), 3),
        ];
        v.sort_by(sort_todos);
        let ids: Vec<&str> = v.iter().map(|t| t.id.as_str()).collect();
        // 미완료(마감시간 오름차순) → 마감없음 → 완료
        assert_eq!(ids, vec!["early", "late", "notime", "done1"]);
    }

    #[test]
    fn pick_color_is_deterministic_and_in_palette() {
        const PALETTE: [&str; 6] = ["yellow", "pink", "blue", "green", "purple", "orange"];
        let c1 = pick_color("회의 준비");
        let c2 = pick_color("회의 준비");
        assert_eq!(c1, c2);
        assert!(PALETTE.contains(&c1.as_str()));
        // ASCII 해시 회귀 고정값 (main.js 알고리즘과 동일해야 함)
        // "A" => charCode 65 => 65 % 6 = 5 => orange
        assert_eq!(pick_color("A"), "orange");
    }

    #[test]
    fn siren_eligibility_respects_ack_and_lead() {
        let now = now_ms();
        // 마감 2분 뒤, 미완료, ack 없음 → 대상
        let mut t = todo("s", false, Some(now + 2 * 60 * 1000), now);
        assert!(siren_eligible(&t, now));
        // ack 처리(= dueAt) → 제외
        t.ack_due = t.due_at;
        assert!(!siren_eligible(&t, now));
        // 완료 → 제외
        let mut d = todo("d", true, Some(now + 60 * 1000), now);
        d.ack_due = None;
        assert!(!siren_eligible(&d, now));
        // 마감 10분 뒤(리드 초과) → 제외
        let far = todo("f", false, Some(now + 10 * 60 * 1000), now);
        assert!(!siren_eligible(&far, now));
    }
}

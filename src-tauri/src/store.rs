// JSON 저장소 — lib/store.js 와 동일 스키마.
// Electron(maxalert-data.json)이 쓴 파일을 Tauri 가 읽고, 그 역도 성립해야 한다.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::logic::now_ms;

const THIRTY_DAYS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awarded: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postpones: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_due: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion_page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion_due_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion_done: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_synced_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_at: Option<i64>,
    /// Electron(윈도우)·브리지가 쓰는 미지의 필드를 저장 시 유실하지 않기 위한 패스스루.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assignee {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostitPos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub notion_token: String,
    #[serde(default)]
    pub notion_db: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion_assignee: Option<Assignee>,
    #[serde(default)]
    pub open_at_login: bool,
    #[serde(default = "default_volume")]
    pub siren_volume: f64,
    #[serde(default = "default_theme")]
    pub postit_theme: String,
    #[serde(default = "default_unlocked")]
    pub unlocked_themes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postit_pos: Option<PostitPos>,
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_volume() -> f64 {
    0.5
}
fn default_theme() -> String {
    "classic".to_string()
}
fn default_unlocked() -> Vec<String> {
    vec!["classic".to_string()]
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            notion_token: String::new(),
            notion_db: String::new(),
            notion_assignee: None,
            open_at_login: false,
            siren_volume: default_volume(),
            postit_theme: default_theme(),
            unlocked_themes: default_unlocked(),
            postit_pos: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Streak {
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub last_date: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub at: i64,
    pub delta: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Points {
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub ledger: Vec<LedgerEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    #[serde(default)]
    pub total_done: i64,
    #[serde(default)]
    pub siren_saves: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BadgeOwned {
    pub id: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub streak: Streak,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reward_date: Option<String>,
    #[serde(default)]
    pub points: Points,
    #[serde(default)]
    pub stats: Stats,
    #[serde(default)]
    pub badges: Vec<BadgeOwned>,
    /// 노션에서 다시 안 받겠다고 사용자가 삭제한 페이지 ID 목록 (브리지가 읽고 건너뜀)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suppressed_notion_ids: Vec<String>,
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Data {
    fn default() -> Self {
        Data {
            todos: Vec::new(),
            settings: Settings::default(),
            streak: Streak::default(),
            last_reward_date: None,
            points: Points::default(),
            stats: Stats::default(),
            badges: Vec::new(),
            suppressed_notion_ids: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

pub struct Store {
    pub file: PathBuf,
    pub data: Data,
    /// 마지막으로 우리가 파일을 쓴 뒤의 mtime — 외부(브리지) 변경 감지용
    pub last_write_mtime: Option<std::time::SystemTime>,
}

impl Store {
    /// primary 가 없으면 electron_src → byeadhd_src 순으로 복사 시도 후 로드한다.
    pub fn load(primary: PathBuf, electron_src: Option<PathBuf>, byeadhd_src: Option<PathBuf>) -> Store {
        if !primary.exists() {
            for src in [electron_src, byeadhd_src].into_iter().flatten() {
                if src.exists() {
                    if let Some(parent) = primary.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::copy(&src, &primary).is_ok() {
                        break;
                    }
                }
            }
        }
        let mut data: Data = match fs::read_to_string(&primary) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Data::default(),
        };
        // 30일 지난 항목 정리 (store.js 와 동일)
        let cutoff = now_ms() - THIRTY_DAYS;
        data.todos.retain(|t| t.created_at >= cutoff);
        let mtime = fs::metadata(&primary).and_then(|m| m.modified()).ok();
        Store { file: primary, data, last_write_mtime: mtime }
    }

    pub fn save(&mut self) {
        if let Some(parent) = self.file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(&self.data) {
            let _ = fs::write(&self.file, s);
        }
        self.last_write_mtime = fs::metadata(&self.file).and_then(|m| m.modified()).ok();
    }

    /// 외부 프로세스(브리지)가 파일을 바꿨으면 디스크 내용으로 다시 읽는다.
    /// 반환값: 다시 읽었는지 여부.
    pub fn reload_if_changed(&mut self) -> bool {
        let disk_mtime = match fs::metadata(&self.file).and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if self.last_write_mtime == Some(disk_mtime) {
            return false;
        }
        match fs::read_to_string(&self.file)
            .ok()
            .and_then(|s| serde_json::from_str::<Data>(&s).ok())
        {
            Some(data) => {
                self.data = data;
                self.last_write_mtime = Some(disk_mtime);
                true
            }
            None => false,
        }
    }
}

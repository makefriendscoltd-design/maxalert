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
        }
    }
}

pub struct Store {
    pub file: PathBuf,
    pub data: Data,
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
        Store { file: primary, data }
    }

    pub fn save(&self) {
        if let Some(parent) = self.file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(&self.data) {
            let _ = fs::write(&self.file, s);
        }
    }
}

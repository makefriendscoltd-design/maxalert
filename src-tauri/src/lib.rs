// MaxAlert — Tauri v2 백엔드.
// Electron main.js 의 창 관리 / 트레이 / 스케줄러 / IPC 를 Tauri 로 이식한다.
mod logic;
mod notion;
mod store;

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    RunEvent, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{ManagerExt as _, MacosLauncher};
use tauri_plugin_clipboard_manager::ClipboardExt;

use logic::{now_ms, today_str, tomorrow_str, SIREN_LEAD_MS};
use store::{Data, PostitPos, Settings, Store, Todo};

const COLORS: [&str; 6] = ["yellow", "pink", "blue", "green", "purple", "orange"];
const AUTOPLAY_RESUME_JS: &str = "window.__maxalertResumeAudio && window.__maxalertResumeAudio();";
#[cfg(not(target_os = "windows"))]
const UPDATE_CHECK_URL: &str = "https://api.aimax.ai.kr/api/workers";
const UPDATE_DOWNLOAD_URL: &str = "https://lounge.aimax.ai.kr";
/// 윈도우 사일런트 자동업데이트 매니페스트 — 파트너의 기존 릴리스 채널(electron-updater latest.yml)을
/// 그대로 소비해 채널 연속성을 유지한다. 실기 검증용 오버라이드: MAXALERT_UPDATE_URL.
#[cfg(target_os = "windows")]
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/makefriendscoltd-design/maxalert-releases/releases/latest/download/latest.yml";

static QUITTING: AtomicBool = AtomicBool::new(false);

// ---------- 전역 상태 ----------
#[derive(Default)]
struct SirenState {
    todo_id: Option<String>,
    labels: Vec<String>,
    generation: u64,
}

struct AppState {
    store: Mutex<Store>,
    schema_cache: Mutex<Option<(String, notion::Schema)>>,
    notion_busy: Mutex<bool>,
    siren: Mutex<SirenState>,
    mini_postits: Mutex<HashMap<String, String>>,
}

// ---------- 헬퍼 ----------
fn rand_suffix() -> String {
    use std::sync::atomic::AtomicU64;
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    let mut v = n ^ c.wrapping_mul(2_654_435_761);
    let chars = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = String::new();
    for _ in 0..4 {
        s.push(chars[(v % 36) as usize] as char);
        v /= 36;
    }
    s
}

fn parse_time_today(time: &str) -> Option<i64> {
    let mut parts = time.split(':');
    let h: u32 = parts.next()?.trim().parse().ok()?;
    let m: u32 = parts.next()?.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    let today = Local::now().date_naive();
    let naive = today.and_hms_opt(h, m, 0)?;
    Local.from_local_datetime(&naive).single().map(|dt| dt.timestamp_millis())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}

fn parse_numeric_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn version_is_newer(candidate: &str, current: &str) -> Option<bool> {
    let candidate = parse_numeric_version(candidate)?;
    let current = parse_numeric_version(current)?;
    Some(candidate > current)
}

#[cfg(not(target_os = "windows"))]
async fn fetch_latest_version(client: &reqwest::Client) -> Option<String> {
    let response = client
        .get(UPDATE_CHECK_URL)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let payload: Value = response.json().await.ok()?;
    let workers = payload.get("workers")?.as_array()?;
    for worker in workers {
        if worker.get("code").and_then(Value::as_str) == Some("max_alert") {
            return worker
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    None
}

/// electron-builder latest.yml 최소 파서 — 최상위 version / path / sha512 만 읽는다.
/// (files: 하위 목록은 들여쓰기라 건너뛴다. 컨트롤된 포맷 전제의 의도적 최소 구현.)
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, PartialEq)]
struct UpdateManifest {
    version: String,
    path: String,
    sha512_b64: String,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_update_manifest(text: &str) -> Option<UpdateManifest> {
    let mut version = None;
    let mut path = None;
    let mut sha512 = None;
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('\'').trim_matches('"');
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "version" => version = Some(value.to_string()),
            "path" => path = Some(value.to_string()),
            "sha512" => sha512 = Some(value.to_string()),
            _ => {}
        }
    }
    Some(UpdateManifest {
        version: version?,
        path: path?,
        sha512_b64: sha512?,
    })
}

/// 매니페스트 URL 과 같은 디렉터리의 파일 URL (GitHub releases/latest/download/ 규약과 테스트 서버 공용)
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn sibling_url(manifest_url: &str, file: &str) -> Option<String> {
    let (base, _) = manifest_url.rsplit_once('/')?;
    Some(format!("{base}/{file}"))
}

/// 업데이트 URL 허용 규칙: https 전부, http 는 로컬 실기 검증용 루프백만.
/// (MAXALERT_UPDATE_URL 오버라이드가 평문 http 원격을 가리키는 MITM 창구가 되지 않게.)
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn update_url_allowed(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
}

/// 윈도우 사일런트 자동업데이트 1회 시도. Electron 시절의 quitAndInstall(true, true) 동작을 계승:
/// 새 버전 발견 → 다운로드 → sha512 검증 → /S 설치 실행.
/// 자진 종료하지 않는다 — 설치기 PREINSTALL 훅의 taskkill 이 앱을 닫는다. 설치기가 그 전에
/// 실패(CRC·WebView2 등)하면 앱은 계속 살아 있고 다음 주기에 재시도한다.
/// 반환 true = 사이렌 발화 중이라 연기 (짧은 주기로 재시도).
#[cfg(target_os = "windows")]
async fn run_silent_update(app: &AppHandle, client: &reqwest::Client, manifest_url: &str) -> bool {
    use base64::Engine as _;
    use sha2::{Digest as _, Sha512};

    fn siren_active(app: &AppHandle) -> bool {
        let state = app.state::<AppState>();
        let active = state
            .siren
            .lock()
            .map(|s| s.todo_id.is_some())
            .unwrap_or(false);
        active
    }

    if !update_url_allowed(manifest_url) {
        return false;
    }
    if siren_active(app) {
        return true; // 마감 3분 전 사이렌을 업데이트 재시작으로 끊지 않는다
    }
    let Ok(response) = client.get(manifest_url).send().await else {
        return false;
    };
    let Ok(response) = response.error_for_status() else {
        return false;
    };
    let Ok(text) = response.text().await else {
        return false;
    };
    let Some(manifest) = parse_update_manifest(&text) else {
        return false;
    };
    let current = app.package_info().version.to_string();
    if version_is_newer(&manifest.version, &current) != Some(true) {
        return false;
    }
    let Some(exe_url) = sibling_url(manifest_url, &manifest.path) else {
        return false;
    };
    let Ok(download) = client.get(&exe_url).send().await else {
        return false;
    };
    let Ok(download) = download.error_for_status() else {
        return false;
    };
    let Ok(bytes) = download.bytes().await else {
        return false;
    };
    let Ok(expected) =
        base64::engine::general_purpose::STANDARD.decode(manifest.sha512_b64.as_bytes())
    else {
        return false;
    };
    if Sha512::digest(&bytes).as_slice() != expected.as_slice() {
        return false; // 해시 불일치 — 절대 설치하지 않는다
    }
    // 다운로드(최대 수 분) 사이에 사이렌이 시작됐을 수 있다 — 실행 직전 재확인
    if siren_active(app) {
        return true;
    }
    // 파일명에 랜덤 접미사 + create_new: 예측 가능한 경로 재사용/치환 창구 축소
    let installer = std::env::temp_dir().join(format!(
        "maxalert-update-{}-{}.exe",
        manifest.version,
        rand_suffix()
    ));
    {
        use std::io::Write as _;
        let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&installer)
        else {
            return false;
        };
        if f.write_all(&bytes).is_err() {
            return false;
        }
    }
    let _ = std::process::Command::new(&installer)
        .args(["/S", "--updated"])
        .spawn();
    false
}

fn spawn_update_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        #[cfg(target_os = "windows")]
        {
            let client = match reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(10 * 60))
                .build()
            {
                Ok(client) => client,
                Err(_) => return,
            };
            let override_url = std::env::var("MAXALERT_UPDATE_URL").ok();
            let test_mode = override_url.is_some();
            let manifest_url = override_url.unwrap_or_else(|| UPDATE_MANIFEST_URL.to_string());
            let (initial, interval) = if test_mode {
                (Duration::from_secs(15), Duration::from_secs(60))
            } else {
                (Duration::from_secs(5 * 60), Duration::from_secs(6 * 60 * 60))
            };
            tokio::time::sleep(initial).await;
            loop {
                let postponed = run_silent_update(&app, &client, &manifest_url).await;
                // 사이렌 때문에 연기됐으면 6시간을 다 기다리지 않고 10분 뒤 재시도
                let delay = if postponed && !test_mode {
                    Duration::from_secs(10 * 60)
                } else {
                    interval
                };
                tokio::time::sleep(delay).await;
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let client = match reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
            {
                Ok(client) => client,
                Err(_) => return,
            };
            let current = app.package_info().version.to_string();
            let mut emitted = HashSet::new();
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
            loop {
                if let Some(version) = fetch_latest_version(&client).await {
                    if version_is_newer(&version, &current) == Some(true)
                        && emitted.insert(version.clone())
                    {
                        let _ = app.emit("update:available", json!({ "version": version }));
                    }
                }
                tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
            }
        }
    });
}


/// id 매칭 시 오늘 날짜 항목을 우선한다 — 브리지가 만든 노션 항목은 과거 완료 기록과
/// 같은 id 를 가질 수 있어(페이지 ID 기반), 첫 매치가 화면 밖 과거 항목을 잡는 사고 방지.
fn find_todo_index(data: &Data, id: &str) -> Option<usize> {
    let today = today_str();
    data.todos
        .iter()
        .position(|t| t.id == id && t.date == today)
        .or_else(|| data.todos.iter().position(|t| t.id == id))
}

fn build_todos_payload(data: &Data) -> Value {
    let today = today_str();
    let mut todos: Vec<Todo> = data.todos.iter().filter(|t| t.date == today).cloned().collect();
    todos.sort_by(logic::sort_todos);
    let theme = if data.settings.postit_theme.is_empty() {
        "classic".to_string()
    } else {
        data.settings.postit_theme.clone()
    };
    let todos_v = serde_json::to_value(&todos).unwrap_or(Value::Null);
    let streak_v = serde_json::to_value(&data.streak).unwrap_or(Value::Null);
    json!({
        "todos": todos_v,
        "now": now_ms(),
        "streak": streak_v,
        "profile": logic::profile_payload(data),
        "theme": theme,
        "focus": data.settings.postit_focus,
    })
}

fn push_todos(app: &AppHandle) {
    let state = app.state::<AppState>();
    let payload = {
        let store = state.store.lock().unwrap();
        build_todos_payload(&store.data)
    };
    let _ = app.emit("todos", payload);
}

// ---------- 창: 대시보드 / 포스트잇 ----------
fn show_dashboard(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("dashboard") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = WebviewWindowBuilder::new(&app2, "dashboard", WebviewUrl::App("dashboard.html".into()))
            .title("MaxAlert")
            .inner_size(540.0, 780.0)
            .build();
    });
}

fn primary_work_area(app: &AppHandle) -> (f64, f64, f64, f64) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        // Electron workArea 대응: 메뉴바·독을 제외한 실사용 영역 (논리좌표 변환)
        let wa = m.work_area();
        return (
            wa.position.x as f64 / scale,
            wa.position.y as f64 / scale,
            wa.size.width as f64 / scale,
            wa.size.height as f64 / scale,
        );
    }
    (0.0, 0.0, 1440.0, 900.0)
}

fn primary_bounds(app: &AppHandle) -> (i32, i32, u32, u32) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let p = m.position();
        let s = m.size();
        return (p.x, p.y, s.width, s.height);
    }
    (0, 0, 1440, 900)
}

fn persist_postit_pos(app: &AppHandle, x: i32, y: i32) {
    let state = app.state::<AppState>();
    let mut store = state.store.lock().unwrap();
    store.data.settings.postit_pos = Some(PostitPos { x, y });
    store.save();
}

fn create_postit_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    let saved = { state.store.lock().unwrap().data.settings.postit_pos.clone() };
    let (mx, my, mw, mh) = primary_work_area(app);
    let width = 340.0_f64;
    let (x, y) = match saved {
        Some(p) => (p.x as f64, p.y as f64),
        None => (mx + mw - width, my),
    };
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(win) =
            WebviewWindowBuilder::new(&app2, "postit", WebviewUrl::App("postit.html".into()))
                .decorations(false)
                .transparent(true)
                .resizable(false)
                .skip_taskbar(true)
                .focused(false)
                .shadow(false)
                .always_on_top(true)
                .accept_first_mouse(true)
                .inner_size(width, mh)
                .position(x, y)
                .build()
        {
            let _ = win.set_ignore_cursor_events(true);
            let handle = app2.clone();
            win.on_window_event(move |ev| {
                if let WindowEvent::Moved(pos) = ev {
                    // Electron 은 논리좌표를 저장하므로 scale 로 나눠 통일한다.
                    let scale = handle
                        .get_webview_window("postit")
                        .and_then(|w| w.scale_factor().ok())
                        .unwrap_or(1.0);
                    persist_postit_pos(
                        &handle,
                        (pos.x as f64 / scale).round() as i32,
                        (pos.y as f64 / scale).round() as i32,
                    );
                }
            });
        }
    });
}

fn monitor_key(m: &tauri::Monitor) -> String {
    if let Some(name) = m.name().filter(|name| !name.is_empty()) {
        return name.clone();
    }
    let pos = m.position();
    format!("{},{}", pos.x, pos.y)
}

fn postit_mini_label(key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("postit-mini-{:016x}", hasher.finish())
}

fn sync_postit_mini_windows(app: &AppHandle) {
    let monitors = app.available_monitors().unwrap_or_default();
    let primary_pos = app.primary_monitor().ok().flatten().map(|m| *m.position());
    let desired: HashMap<String, (String, tauri::Monitor)> = monitors
        .into_iter()
        .enumerate()
        .filter_map(|(i, monitor)| {
            let is_primary = primary_pos
                .map(|pos| pos == *monitor.position())
                .unwrap_or(i == 0);
            if is_primary {
                return None;
            }
            let key = monitor_key(&monitor);
            Some((postit_mini_label(&key), (key, monitor)))
        })
        .collect();
    let desired_labels: HashSet<String> = desired.keys().cloned().collect();
    let state = app.state::<AppState>();

    let stale_labels = {
        let mut tracked = state.mini_postits.lock().unwrap();
        let stale: Vec<String> = tracked
            .keys()
            .filter(|label| !desired_labels.contains(*label))
            .cloned()
            .collect();
        for label in &stale {
            tracked.remove(label);
        }
        stale
    };
    for label in stale_labels {
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(win) = app2.get_webview_window(&label) {
                let _ = win.destroy();
            }
        });
    }

    let saved_positions = {
        state
            .store
            .lock()
            .unwrap()
            .data
            .settings
            .postit_mini_pos
            .clone()
    };

    for (label, (key, monitor)) in desired {
        let already_exists = {
            let mut tracked = state.mini_postits.lock().unwrap();
            if tracked.contains_key(&label) && app.get_webview_window(&label).is_some() {
                true
            } else {
                tracked.insert(label.clone(), key.clone());
                false
            }
        };
        if already_exists {
            continue;
        }

        let scale = monitor.scale_factor();
        let monitor_pos = *monitor.position();
        let monitor_size = *monitor.size();
        let work_area = *monitor.work_area();
        let width = (340.0 * scale).round() as u32;
        let default_pos = PhysicalPosition::new(
            work_area.position.x + work_area.size.width as i32 - width as i32,
            work_area.position.y,
        );
        let pos = saved_positions
            .get(&key)
            .map(|saved| {
                PhysicalPosition::new(
                    (saved.x as f64 * scale).round() as i32,
                    (saved.y as f64 * scale).round() as i32,
                )
            })
            .filter(|saved| {
                saved.x >= monitor_pos.x
                    && saved.y >= monitor_pos.y
                    && saved.x < monitor_pos.x + monitor_size.width as i32
                    && saved.y < monitor_pos.y + monitor_size.height as i32
            })
            .unwrap_or(default_pos);
        let size = PhysicalSize::new(width, work_area.size.height);
        let app2 = app.clone();
        let label2 = label.clone();
        let _ = app.run_on_main_thread(move || {
            let still_desired = app2
                .state::<AppState>()
                .mini_postits
                .lock()
                .unwrap()
                .get(&label2)
                == Some(&key);
            if !still_desired {
                return;
            }
            if let Ok(win) = WebviewWindowBuilder::new(
                &app2,
                label2.clone(),
                WebviewUrl::App("postit.html?mini=1".into()),
            )
            .decorations(false)
            .transparent(true)
            .resizable(false)
            .skip_taskbar(true)
            .focused(false)
            .shadow(false)
            .always_on_top(true)
            .accept_first_mouse(true)
            .visible(false)
            .build()
            {
                let _ = win.set_position(pos);
                let _ = win.set_size(size);
                let _ = win.set_ignore_cursor_events(true);
                let should_show = app2
                    .get_webview_window("postit")
                    .map(|main| main.is_visible().unwrap_or(true))
                    .unwrap_or(true);
                if should_show {
                    let _ = win.show();
                }
                let handle = app2.clone();
                let event_label = label2.clone();
                win.on_window_event(move |ev| {
                    if let WindowEvent::Moved(pos) = ev {
                        let scale = handle
                            .get_webview_window(&event_label)
                            .and_then(|w| w.scale_factor().ok())
                            .unwrap_or(1.0);
                        let state = handle.state::<AppState>();
                        let mut store = state.store.lock().unwrap();
                        store.data.settings.postit_mini_pos.insert(
                            key.clone(),
                            PostitPos {
                                x: (pos.x as f64 / scale).round() as i32,
                                y: (pos.y as f64 / scale).round() as i32,
                            },
                        );
                        store.save();
                    }
                });
            }
        });
    }
}

// macOS Tauri 는 ignore_cursor_events 상태에서 Electron 의 forward 처럼 mousemove 를
// 웹뷰에 전달해주지 않는다. 그래서 클릭스루 해제 판정에 쓸 커서 좌표를 Rust 가
// 전역 폴링해 postit 웹뷰로 밀어준다. 판정(.interactive 위인지)은 api-tauri.js
// 한 곳에서만 한다 — Rust 는 좌표 공급과 토글 실행만 담당.
fn spawn_postit_cursor_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(80));
        let mut window_states: HashMap<String, (bool, Option<(i32, i32)>)> = HashMap::new();
        loop {
            interval.tick().await;
            let windows: Vec<(String, tauri::WebviewWindow)> = app
                .webview_windows()
                .into_iter()
                .filter(|(label, _)| label.starts_with("postit"))
                .collect();
            let labels: HashSet<&str> = windows.iter().map(|(label, _)| label.as_str()).collect();
            window_states.retain(|label, _| labels.contains(label.as_str()));
            if windows.is_empty() {
                continue;
            }
            let Ok(cursor) = app.cursor_position() else {
                continue;
            };
            for (label, win) in windows {
                if !win.is_visible().unwrap_or(false) {
                    continue;
                }
                let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
                    continue;
                };
                let inside = cursor.x >= pos.x as f64
                    && cursor.y >= pos.y as f64
                    && cursor.x < pos.x as f64 + size.width as f64
                    && cursor.y < pos.y as f64 + size.height as f64;
                let (was_inside, last_sent) =
                    window_states.entry(label.clone()).or_insert((false, None));
                if inside {
                    let scale = win.scale_factor().unwrap_or(1.0);
                    let lx = ((cursor.x - pos.x as f64) / scale).round() as i32;
                    let ly = ((cursor.y - pos.y as f64) / scale).round() as i32;
                    if *last_sent != Some((lx, ly)) {
                        *last_sent = Some((lx, ly));
                        let _ = app.emit_to(&label, "postit:cursor", json!({ "x": lx, "y": ly }));
                    }
                    *was_inside = true;
                } else if *was_inside {
                    *was_inside = false;
                    *last_sent = None;
                    let _ = app.emit_to(&label, "postit:cursor", json!({ "x": -1, "y": -1 }));
                }
            }
        }
    });
}

fn toggle_postit(app: &AppHandle) {
    match app.get_webview_window("postit") {
        Some(win) => {
            let visible = win.is_visible().unwrap_or(true);
            for (label, postit) in app.webview_windows() {
                if label.starts_with("postit") {
                    if visible {
                        let _ = postit.hide();
                    } else {
                        let _ = postit.show();
                    }
                }
            }
        }
        None => {
            create_postit_window(app);
            sync_postit_mini_windows(app);
        }
    }
}

// ---------- 창: 사이렌 ----------
fn siren_open(app: &AppHandle, todo: &Todo) {
    let state = app.state::<AppState>();
    let (volume, stage) = {
        let store = state.store.lock().unwrap();
        (
            store.data.settings.siren_volume,
            logic::level_info(store.data.points.total).stage,
        )
    };
    let gen = {
        let mut s = state.siren.lock().unwrap();
        s.generation += 1;
        s.todo_id = Some(todo.id.clone());
        s.generation
    };
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        // 모니터 열거(NSScreen)는 메인 스레드에서 — 비메인 스레드 호출은 목록 누락 위험
        let st = app2.state::<AppState>();
        {
            let s = st.siren.lock().unwrap();
            if s.generation != gen {
                return; // 창 생성 전에 이미 닫힘
            }
        }
        let monitors = app2.available_monitors().unwrap_or_default();
        let primary_pos = app2.primary_monitor().ok().flatten().map(|m| *m.position());
        let mut labels = Vec::new();
        for (i, m) in monitors.iter().enumerate() {
            let label = format!("siren-{}", i);
            let is_primary = primary_pos.map(|p| p == *m.position()).unwrap_or(i == 0);
            let url = format!(
                "siren.html?sound={}&volume={}&stage={}",
                if is_primary { 1 } else { 0 },
                volume,
                urlencode(&stage)
            );
            // macOS 전역 좌표는 포인트(논리) 기준 — 모니터별 배율이 섞이면(내장 2x+외장 1x)
            // 물리픽셀 배치는 보조 모니터 창이 주 모니터 위에 겹친다 (Electron d.bounds=DIP와 동일하게 논리 사용)
            let scale = m.scale_factor();
            let pos = (*m.position()).to_logical::<f64>(scale);
            let size = (*m.size()).to_logical::<f64>(scale);
            if let Ok(win) =
                WebviewWindowBuilder::new(&app2, label.clone(), WebviewUrl::App(url.into()))
                    .decorations(false)
                    .skip_taskbar(true)
                    .always_on_top(true)
                    .resizable(false)
                    .closable(false)
                    .minimizable(false)
                    .visible(false)
                    .accept_first_mouse(true)
                    .build()
            {
                let _ = win.set_position(LogicalPosition::new(pos.x, pos.y));
                let _ = win.set_size(LogicalSize::new(size.width, size.height));
                let _ = win.show();
                let _ = win.set_focus();
                let _ = win.set_always_on_top(true);
                // 오디오 자동재생 폴백 (JS AudioContext resume 훅 호출)
                let _ = win.eval(AUTOPLAY_RESUME_JS);
            }
            labels.push(label);
        }
        let mut s = st.siren.lock().unwrap();
        if s.generation == gen {
            s.labels = labels;
        } else {
            // 생성 중 siren_close가 지나감 — 방금 만든 창 정리
            drop(s);
            for l in labels {
                if let Some(w) = app2.get_webview_window(&l) {
                    let _ = w.destroy();
                }
            }
        }
    });
    let _ = app.emit("siren:todo", todo);
    spawn_focus_timer(app.clone(), gen);
}

fn siren_close(app: &AppHandle) {
    let state = app.state::<AppState>();
    let labels = {
        let mut s = state.siren.lock().unwrap();
        s.generation += 1;
        s.todo_id = None;
        std::mem::take(&mut s.labels)
    };
    for l in labels {
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = app2.get_webview_window(&l) {
                let _ = w.destroy();
            }
        });
    }
}

fn siren_close_if_target(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let is = { state.siren.lock().unwrap().todo_id.as_deref() == Some(id) };
    if is {
        siren_close(app);
    }
}

fn spawn_focus_timer(app: AppHandle, gen: u64) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(700));
        loop {
            interval.tick().await;
            let (labels, active) = {
                let state = app.state::<AppState>();
                let s = state.siren.lock().unwrap();
                (s.labels.clone(), s.generation == gen && s.todo_id.is_some())
            };
            if !active {
                break;
            }
            for l in labels {
                if let Some(win) = app.get_webview_window(&l) {
                    let _ = win.set_always_on_top(true);
                    let _ = win.set_focus();
                }
            }
        }
    });
}

// ---------- 창: 보상 ----------
fn open_reward(app: &AppHandle, info: logic::RewardInfo) {
    if let Some(win) = app.get_webview_window("reward") {
        let _ = win.destroy();
    }
    let (px, py, pw, ph) = primary_bounds(app);
    let url = format!(
        "reward.html?streak={}&today={}&total={}&title={}&icon={}&stage={}&next={}&min={}",
        info.streak,
        info.today,
        info.total,
        urlencode(&info.title),
        urlencode(&info.icon),
        urlencode(&info.stage),
        urlencode(&info.next),
        info.min
    );
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(win) = WebviewWindowBuilder::new(&app2, "reward", WebviewUrl::App(url.into()))
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .build()
        {
            let _ = win.set_position(PhysicalPosition::new(px, py));
            let _ = win.set_size(PhysicalSize::new(pw, ph));
            let _ = win.show();
            let _ = win.set_always_on_top(true);
        }
    });
    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if let Some(win) = app3.get_webview_window("reward") {
            let _ = win.destroy();
        }
    });
}

// ---------- 노션 ----------
async fn get_schema_for(
    app: &AppHandle,
    token: &str,
    db: &str,
) -> Result<notion::Schema, String> {
    let state = app.state::<AppState>();
    let tail = if token.len() > 8 { &token[token.len() - 8..] } else { token };
    let key = format!("{}:{}", tail, db);
    {
        let cache = state.schema_cache.lock().unwrap();
        if let Some((k, s)) = cache.as_ref() {
            if k == &key {
                return Ok(s.clone());
            }
        }
    }
    let schema = notion::get_schema(token, db).await?;
    *state.schema_cache.lock().unwrap() = Some((key, schema.clone()));
    Ok(schema)
}

fn push_notion_done(app: &AppHandle, id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let (token, db, page_id, done) = {
            let store = state.store.lock().unwrap();
            let s = &store.data.settings;
            match store.data.todos.iter().find(|t| t.id == id) {
                Some(t)
                    if !s.notion_token.is_empty()
                        && !s.notion_db.is_empty()
                        && t.notion_page_id.is_some() =>
                {
                    (
                        s.notion_token.clone(),
                        s.notion_db.clone(),
                        t.notion_page_id.clone().unwrap(),
                        t.done,
                    )
                }
                _ => return,
            }
        };
        let schema = match get_schema_for(&app, &token, &db).await {
            Ok(s) => s,
            Err(_) => return,
        };
        if notion::set_done(&token, &page_id, &schema.done_prop, done).await.is_ok() {
            let state = app.state::<AppState>();
            let mut store = state.store.lock().unwrap();
            if let Some(t) = store.data.todos.iter_mut().find(|t| t.id == id) {
                t.notion_done = Some(done);
            }
            store.save();
        }
    });
}

async fn do_sync(app: &AppHandle) -> Value {
    let (token, db, assignee_id) = {
        let state = app.state::<AppState>();
        let store = state.store.lock().unwrap();
        let s = &store.data.settings;
        (
            s.notion_token.clone(),
            s.notion_db.clone(),
            s.notion_assignee.as_ref().map(|a| a.id.clone()),
        )
    };
    if token.is_empty() || db.is_empty() {
        return json!({ "ok": false, "error": "노션 토큰/DB가 설정되지 않았습니다" });
    }
    {
        let state = app.state::<AppState>();
        let mut busy = state.notion_busy.lock().unwrap();
        if *busy {
            return json!({ "ok": false, "error": "동기화 진행 중" });
        }
        *busy = true;
    }
    let result = do_sync_inner(app, &token, &db, assignee_id.as_deref()).await;
    {
        let state = app.state::<AppState>();
        *state.notion_busy.lock().unwrap() = false;
    }
    match result {
        Ok(v) => v,
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

async fn do_sync_inner(
    app: &AppHandle,
    token: &str,
    db: &str,
    assignee_id: Option<&str>,
) -> Result<Value, String> {
    let schema = get_schema_for(app, token, db).await?;
    let today = today_str();
    let tomorrow = tomorrow_str();
    let pages = notion::query_day(token, db, &schema, &today, &tomorrow, assignee_id).await?;
    let now = now_ms();
    let mut added = 0i64;
    let removed;
    let mut reward: Option<logic::RewardInfo> = None;
    let mut to_push_done: Vec<(String, bool)> = Vec::new();
    {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().unwrap();
        for p in &pages {
            let existing = store
                .data
                .todos
                .iter()
                .position(|t| t.notion_page_id.as_deref() == Some(p.id.as_str()));
            match existing {
                None => {
                    let mut t = Todo {
                        id: format!("n{}{}", now, rand_suffix()),
                        title: p.title.clone(),
                        color: logic::pick_color(&p.title),
                        date: today.clone(),
                        done: p.done,
                        created_at: now,
                        due_at: p.due_at,
                        done_at: None,
                        awarded: None,
                        postpones: None,
                        ack_due: None,
                        notion_page_id: Some(p.id.clone()),
                        notion_due_at: p.due_at,
                        notion_done: Some(p.done),
                        bridge_source: None,
                        bridge_synced_at: None,
                        deferred_from: None,
                        deferred_at: None,
                        extra: Default::default(),
                    };
                    if let Some(d) = t.due_at {
                        if d < now {
                            t.ack_due = Some(d);
                        }
                    }
                    store.data.todos.push(t);
                    added += 1;
                }
                Some(i) => {
                    store.data.todos[i].title = p.title.clone();
                    if p.due_at != store.data.todos[i].notion_due_at {
                        store.data.todos[i].due_at = p.due_at;
                        store.data.todos[i].notion_due_at = p.due_at;
                        store.data.todos[i].ack_due = None;
                    }
                    let notion_done = store.data.todos[i].notion_done;
                    if Some(p.done) != notion_done {
                        store.data.todos[i].done = p.done;
                        store.data.todos[i].notion_done = Some(p.done);
                        let title = store.data.todos[i].title.clone();
                        if p.done {
                            store.data.todos[i].done_at = Some(now);
                            store.data.todos[i].awarded = Some(10);
                            logic::add_points(&mut store.data, 10, &format!("완료(노션): {}", title));
                            store.data.stats.total_done += 1;
                            logic::check_badges(&mut store.data);
                            if let Some(r) = logic::maybe_reward(&mut store.data) {
                                reward = Some(r);
                            }
                        } else {
                            store.data.todos[i].done_at = None;
                            let awarded = store.data.todos[i].awarded.unwrap_or(10);
                            logic::add_points(
                                &mut store.data,
                                -awarded,
                                &format!("완료 취소(노션): {}", title),
                            );
                            store.data.todos[i].awarded = Some(0);
                            store.data.stats.total_done = (store.data.stats.total_done - 1).max(0);
                        }
                    } else if store.data.todos[i].done != p.done {
                        to_push_done.push((p.id.clone(), store.data.todos[i].done));
                        store.data.todos[i].notion_done = Some(store.data.todos[i].done);
                    }
                }
            }
        }
        let returned: std::collections::HashSet<&str> =
            pages.iter().map(|p| p.id.as_str()).collect();
        let before = store.data.todos.len();
        store.data.todos.retain(|t| {
            if let Some(pid) = &t.notion_page_id {
                if t.date == today && !returned.contains(pid.as_str()) {
                    return false;
                }
            }
            true
        });
        removed = (before - store.data.todos.len()) as i64;
        store.save();
    }
    for (page_id, done) in to_push_done {
        let _ = notion::set_done(token, &page_id, &schema.done_prop, done).await;
    }
    push_todos(app);
    if let Some(info) = reward {
        open_reward(app, info);
    }
    Ok(json!({ "ok": true, "count": pages.len(), "added": added, "removed": removed, "at": now_ms() }))
}

// ---------- 스케줄러 ----------
fn spawn_tick(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut count: u64 = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            count += 1;
            do_tick(&app, count);
        }
    });
}

fn do_tick(app: &AppHandle, count: u64) {
    let now = now_ms();
    let today = today_str();
    let todos_today: Vec<Todo> = {
        let state = app.state::<AppState>();
        let store = state.store.lock().unwrap();
        store
            .data
            .todos
            .iter()
            .filter(|t| t.date == today)
            .cloned()
            .collect()
    };
    let cur_target = {
        let state = app.state::<AppState>();
        let id = state.siren.lock().unwrap().todo_id.clone();
        id
    };
    if let Some(id) = cur_target {
        match todos_today.iter().find(|t| t.id == id) {
            Some(cur) if logic::siren_eligible(cur, now) => {
                let _ = app.emit("siren:todo", cur);
            }
            _ => siren_close(app),
        }
    } else {
        let target = todos_today
            .iter()
            .filter(|t| logic::siren_eligible(t, now))
            .min_by_key(|t| t.due_at.unwrap_or(i64::MAX))
            .cloned();
        if let Some(t) = target {
            siren_open(app, &t);
        }
    }
    if count % 5 == 0 {
        sync_postit_mini_windows(app);
        // 다른 프로그램이 topmost 를 뺏으면 위젯이 창 뒤로 가려짐 → 주기 재선언 (윈도우 v0.1.14 동작)
        for (label, win) in app.webview_windows() {
            if label.starts_with("postit") {
                let _ = win.set_always_on_top(true);
            }
        }
        // 브리지가 파일을 바꿨으면 디스크 기준으로 다시 읽는다 (아침 동기화 유실 방지)
        let reloaded = {
            let state = app.state::<AppState>();
            let mut store = state.store.lock().unwrap();
            store.reload_if_changed()
        };
        if reloaded {
            push_todos(app);
        }
    }
    if count % 15 == 0 {
        push_todos(app);
    }
}

fn spawn_notion_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let _ = do_sync(&app).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await; // 첫 tick 즉시 소비 (방금 sync 완료)
        loop {
            interval.tick().await;
            let _ = do_sync(&app).await;
        }
    });
}

// ---------- 자동 시작 ----------
fn apply_autostart(app: &AppHandle, enabled: bool) {
    let mgr = app.autolaunch();
    if enabled {
        let _ = mgr.enable();
    } else {
        let _ = mgr.disable();
    }
}

// ---------- 트레이 ----------
fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let icon = tauri::include_image!("icons/icon.png");
    let menu = MenuBuilder::new(app)
        .text("open_dash", "투두리스트 열기")
        .text("toggle_postit", "포스트잇 보이기/숨기기")
        .text("sync_now", "지금 노션 동기화")
        .text("restore_hidden", "숨긴 노션 일정 복원")
        .separator()
        .text("quit", "종료")
        .build()?;
    let _tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("MaxAlert — 일정 사이렌")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_dash" => show_dashboard(app),
            "toggle_postit" => toggle_postit(app),
            "sync_now" => {
                let a = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = do_sync(&a).await;
                });
            }
            "restore_hidden" => {
                let state = app.state::<AppState>();
                {
                    let mut store = state.store.lock().unwrap();
                    store.data.suppressed_notion_ids.clear();
                    store.save();
                }
                push_todos(app);
            }
            "quit" => {
                QUITTING.store(true, Ordering::SeqCst);
                siren_close(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_dashboard(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

// ---------- IPC 커맨드 ----------
#[derive(Deserialize)]
struct AddPayload {
    title: String,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[tauri::command]
fn todos_list(state: State<AppState>) -> Value {
    let store = state.store.lock().unwrap();
    build_todos_payload(&store.data)
}

#[tauri::command]
fn todos_add(state: State<AppState>, app: AppHandle, payload: AddPayload) -> Option<Todo> {
    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let color = match payload.color {
        Some(c) if COLORS.contains(&c.as_str()) => c,
        _ => "yellow".to_string(),
    };
    let now = now_ms();
    let mut t = Todo {
        id: format!("t{}{}", now, rand_suffix()),
        title,
        color,
        date: today_str(),
        done: false,
        created_at: now,
        due_at: None,
        done_at: None,
        awarded: None,
        postpones: None,
        ack_due: None,
        notion_page_id: None,
        notion_due_at: None,
        notion_done: None,
        bridge_source: None,
        bridge_synced_at: None,
        deferred_from: None,
        deferred_at: None,
        extra: Default::default(),
    };
    if let Some(time) = payload.time.filter(|s| !s.is_empty()) {
        if let Some(due) = parse_time_today(&time) {
            t.due_at = Some(due);
            if due < now {
                t.ack_due = Some(due);
            }
        }
    }
    {
        let mut store = state.store.lock().unwrap();
        store.data.todos.push(t.clone());
        store.save();
    }
    push_todos(&app);
    Some(t)
}

#[tauri::command]
fn todos_update(state: State<AppState>, app: AppHandle, id: String, patch: Value) -> Option<Todo> {
    let clone;
    {
        let mut store = state.store.lock().unwrap();
        let now = now_ms();
        let idx = match find_todo_index(&store.data, &id) {
            Some(i) => i,
            None => return None,
        };
        if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
            let tr = title.trim();
            if !tr.is_empty() {
                store.data.todos[idx].title = tr.to_string();
            }
        }
        if let Some(color) = patch.get("color").and_then(|v| v.as_str()) {
            if COLORS.contains(&color) {
                store.data.todos[idx].color = color.to_string();
            }
        }
        if patch.get("time").is_some() {
            let time = patch.get("time").and_then(|v| v.as_str()).unwrap_or("");
            if !time.is_empty() {
                if let Some(due) = parse_time_today(time) {
                    store.data.todos[idx].due_at = Some(due);
                    if due < now {
                        store.data.todos[idx].ack_due = Some(due);
                    } else {
                        store.data.todos[idx].ack_due = None;
                    }
                }
            } else {
                store.data.todos[idx].due_at = None;
                store.data.todos[idx].ack_due = None;
            }
        }
        store.save();
        clone = store.data.todos[idx].clone();
    }
    push_todos(&app);
    Some(clone)
}

#[tauri::command]
fn todos_toggle(state: State<AppState>, app: AppHandle, id: String) -> Option<Todo> {
    let new_done;
    let want_notion;
    let mut reward: Option<logic::RewardInfo> = None;
    let clone;
    {
        let mut store = state.store.lock().unwrap();
        let now = now_ms();
        let idx = match find_todo_index(&store.data, &id) {
            Some(i) => i,
            None => return None,
        };
        new_done = !store.data.todos[idx].done;
        store.data.todos[idx].done = new_done;
        if new_done {
            store.data.todos[idx].done_at = Some(now);
            let due = store.data.todos[idx].due_at;
            let title = store.data.todos[idx].title.clone();
            let mut pts = 10i64;
            let mut reason = format!("완료: {}", title);
            let mut siren_save = false;
            if let Some(d) = due {
                if now <= d {
                    pts += 10;
                    reason.push_str(" (정시 +10)");
                    if d - now <= SIREN_LEAD_MS {
                        siren_save = true;
                    }
                }
            }
            store.data.todos[idx].awarded = Some(pts);
            logic::add_points(&mut store.data, pts, &reason);
            store.data.stats.total_done += 1;
            if siren_save {
                store.data.stats.siren_saves += 1;
            }
            logic::check_badges(&mut store.data);
        } else {
            store.data.todos[idx].done_at = None;
            let awarded = store.data.todos[idx].awarded.unwrap_or(10);
            let title = store.data.todos[idx].title.clone();
            store.data.todos[idx].awarded = Some(0);
            logic::add_points(&mut store.data, -awarded, &format!("완료 취소: {}", title));
            store.data.stats.total_done = (store.data.stats.total_done - 1).max(0);
        }
        want_notion = store.data.todos[idx].notion_page_id.is_some();
        store.save();
        clone = store.data.todos[idx].clone();
        if new_done {
            reward = logic::maybe_reward(&mut store.data);
            if reward.is_some() {
                store.save();
            }
        }
    }
    push_todos(&app);
    if new_done {
        siren_close_if_target(&app, &id);
    }
    if want_notion {
        push_notion_done(&app, id.clone());
    }
    if let Some(info) = reward {
        open_reward(&app, info);
    }
    Some(clone)
}

#[tauri::command]
fn todos_delete(state: State<AppState>, app: AppHandle, id: String) -> bool {
    {
        let mut store = state.store.lock().unwrap();
        if let Some(i) = find_todo_index(&store.data, &id) {
            let t = store.data.todos.remove(i);
            // 브리지(노션) 항목을 지우면 다시 안 받도록 억제 목록에 올린다
            if t.bridge_source.as_deref().map(|s| s.starts_with("notion-")).unwrap_or(false) {
                if let Some(pid) = t.notion_page_id {
                    if !store.data.suppressed_notion_ids.contains(&pid) {
                        store.data.suppressed_notion_ids.push(pid);
                    }
                }
            }
        }
        store.save();
    }
    push_todos(&app);
    true
}

#[tauri::command]
fn todos_postpone(
    state: State<AppState>,
    app: AppHandle,
    id: String,
    minutes: Option<f64>,
) -> Option<Todo> {
    let clone;
    {
        let mut store = state.store.lock().unwrap();
        let now = now_ms();
        let idx = match find_todo_index(&store.data, &id) {
            Some(i) => i,
            None => return None,
        };
        let min = {
            let m = minutes.unwrap_or(10.0);
            let m = if m.is_nan() { 10.0 } else { m };
            (m as i64).max(1)
        };
        let base = now.max(store.data.todos[idx].due_at.unwrap_or(0));
        store.data.todos[idx].due_at = Some(base + min * 60000);
        store.data.todos[idx].postpones = Some(store.data.todos[idx].postpones.unwrap_or(0) + 1);
        store.data.todos[idx].ack_due = None;
        let title = store.data.todos[idx].title.clone();
        logic::add_points(&mut store.data, -3, &format!("미루기: {}", title));
        store.save();
        clone = store.data.todos[idx].clone();
    }
    push_todos(&app);
    {
        let state = app.state::<AppState>();
        let is_target = { state.siren.lock().unwrap().todo_id.as_deref() == Some(id.as_str()) };
        if is_target && !logic::siren_eligible(&clone, now_ms()) {
            siren_close(&app);
        }
    }
    Some(clone)
}

#[tauri::command]
fn todos_postpone_next_weekday(
    state: State<AppState>,
    app: AppHandle,
    id: String,
) -> Option<Todo> {
    let clone;
    {
        let mut store = state.store.lock().unwrap();
        let now = now_ms();
        let idx = match find_todo_index(&store.data, &id) {
            Some(i) => i,
            None => return None,
        };
        if store.data.todos[idx].done {
            return Some(store.data.todos[idx].clone());
        }
        let from_date = if store.data.todos[idx].date.is_empty() {
            today_str()
        } else {
            store.data.todos[idx].date.clone()
        };
        let target_date = logic::next_weekday_after(&from_date)
            .or_else(|| logic::next_weekday_after(&today_str()))
            .unwrap_or_else(tomorrow_str);
        store.data.todos[idx].date = target_date.clone();
        store.data.todos[idx].due_at = store.data.todos[idx]
            .due_at
            .and_then(|due| logic::same_local_time_on_date(due, &target_date));
        store.data.todos[idx].postpones = Some(store.data.todos[idx].postpones.unwrap_or(0) + 1);
        store.data.todos[idx].ack_due = None;
        store.data.todos[idx].deferred_from = Some(from_date);
        store.data.todos[idx].deferred_at = Some(now);
        store.save();
        clone = store.data.todos[idx].clone();
    }
    push_todos(&app);
    siren_close_if_target(&app, &id);
    Some(clone)
}

#[tauri::command]
fn settings_get(state: State<AppState>, app: AppHandle) -> Value {
    let settings = state.store.lock().unwrap().data.settings.clone();
    let mut v = serde_json::to_value(&settings).unwrap_or(json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("appVersion".into(), json!(app.package_info().version.to_string()));
    }
    v
}

#[tauri::command]
fn settings_set(state: State<AppState>, app: AppHandle, patch: Value) -> Settings {
    let (settings, open) = {
        let mut store = state.store.lock().unwrap();
        let mut cur = serde_json::to_value(&store.data.settings).unwrap_or(json!({}));
        if let (Some(cm), Some(pm)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in pm {
                cm.insert(k.clone(), v.clone());
            }
        }
        if let Ok(news) = serde_json::from_value::<Settings>(cur) {
            store.data.settings = news;
        }
        store.save();
        let open = store.data.settings.open_at_login;
        (store.data.settings.clone(), open)
    };
    *state.schema_cache.lock().unwrap() = None;
    apply_autostart(&app, open);
    settings
}

#[tauri::command]
async fn notion_sync(app: AppHandle) -> Result<Value, String> {
    Ok(do_sync(&app).await)
}

#[tauri::command]
async fn notion_users(app: AppHandle) -> Result<Value, String> {
    let token = {
        let state = app.state::<AppState>();
        let store = state.store.lock().unwrap();
        store.data.settings.notion_token.clone()
    };
    if token.is_empty() {
        return Ok(json!({ "ok": false, "error": "노션 토큰을 먼저 입력하세요" }));
    }
    match notion::list_users(&token).await {
        Ok(users) => {
            let arr: Vec<Value> = users
                .into_iter()
                .map(|u| json!({ "id": u.id, "name": u.name }))
                .collect();
            Ok(json!({ "ok": true, "users": arr }))
        }
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

#[tauri::command]
fn report_copy(state: State<AppState>, app: AppHandle) -> Value {
    let md = { logic::build_daily_report(&state.store.lock().unwrap().data) };
    let _ = app.clipboard().write_text(md.clone());
    json!({ "ok": true, "text": md })
}

#[tauri::command]
fn shop_buy_theme(state: State<AppState>, app: AppHandle, id: String) -> Value {
    let res = {
        let mut store = state.store.lock().unwrap();
        let cost = match logic::theme_price(&id) {
            Some(c) => c,
            None => return json!({ "ok": false, "error": "알 수 없는 테마" }),
        };
        if store.data.settings.unlocked_themes.is_empty() {
            store.data.settings.unlocked_themes = vec!["classic".to_string()];
        }
        if !store.data.settings.unlocked_themes.iter().any(|t| t == &id) {
            if store.data.points.total < cost {
                return json!({ "ok": false, "error": format!("포인트 부족 ({}P 필요)", cost) });
            }
            logic::add_points(&mut store.data, -cost, &format!("테마 구입: {}", id));
            store.data.settings.unlocked_themes.push(id.clone());
        }
        store.data.settings.postit_theme = id.clone();
        store.save();
        json!({ "ok": true, "settings": serde_json::to_value(&store.data.settings).unwrap_or(Value::Null) })
    };
    push_todos(&app);
    res
}

#[tauri::command]
fn dashboard_open(app: AppHandle) -> bool {
    show_dashboard(&app);
    true
}

#[tauri::command]
fn app_quit(app: AppHandle) -> bool {
    QUITTING.store(true, Ordering::SeqCst);
    siren_close(&app);
    app.exit(0);
    true
}

#[tauri::command]
fn reward_close(app: AppHandle) -> bool {
    if let Some(win) = app.get_webview_window("reward") {
        let _ = win.destroy();
    }
    true
}

#[tauri::command]
fn postit_mouse(window: tauri::WebviewWindow, ignore: bool) -> bool {
    let _ = window.set_ignore_cursor_events(ignore);
    true
}

#[tauri::command]
fn postit_drag_start(window: tauri::WebviewWindow) -> bool {
    // 지은 v0.2.1 확정 해법: 네이티브 start_dragging() 1회 호출 (커서 추적 타이머 금지)
    let _ = window.start_dragging();
    true
}

#[tauri::command]
fn postit_drag_end() -> bool {
    true
}

#[tauri::command]
fn open_external(url: String) -> bool {
    // prefix 일치만으로는 lounge.aimax.ai.kr.evil.com 류가 통과한다 — 호스트 경계까지 확인.
    let allowed = match url.strip_prefix(UPDATE_DOWNLOAD_URL) {
        Some(rest) => rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#'),
        None => false,
    };
    if !allowed {
        return false;
    }
    std::process::Command::new("open").arg(url).spawn().is_ok()
}

#[tauri::command]
fn open_siren(app: AppHandle, todo: Value) -> bool {
    if let Ok(t) = serde_json::from_value::<Todo>(todo) {
        siren_open(&app, &t);
    }
    true
}

#[tauri::command]
fn close_siren(app: AppHandle) -> bool {
    siren_close(&app);
    true
}

// ---------- 진입점 ----------
pub fn run() {
    // Windows WebView2: 사이렌은 사용자 제스처 없이 소리가 나야 한다 (Electron autoplay-policy 스위치 대응).
    // 웹뷰 환경 생성 전에 설정해야 반영된다. 외부에서 준 인자(원격 디버깅 등)는 덮어쓰지 않고 이어붙인다.
    #[cfg(windows)]
    {
        let mut args = String::from("--autoplay-policy=no-user-gesture-required");
        if let Ok(extra) = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
            let extra = extra.trim();
            if !extra.is_empty() {
                args.push(' ');
                args.push_str(extra);
            }
        }
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &args);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_dashboard(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .invoke_handler(tauri::generate_handler![
            todos_list,
            todos_add,
            todos_update,
            todos_toggle,
            todos_delete,
            todos_postpone,
            todos_postpone_next_weekday,
            settings_get,
            settings_set,
            notion_sync,
            notion_users,
            report_copy,
            shop_buy_theme,
            dashboard_open,
            app_quit,
            reward_close,
            postit_mouse,
            postit_drag_start,
            postit_drag_end,
            open_external,
            open_siren,
            close_siren
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // 데이터 경로 + Electron/byeadhd 마이그레이션
            let data_dir = handle.path().app_data_dir().map_err(|e| e.to_string())?;
            let primary = data_dir.join("maxalert-data.json");
            let base = handle.path().data_dir().ok();
            let electron_src = base
                .as_ref()
                .map(|b| b.join("maxalert").join("maxalert-data.json"));
            let byeadhd_src = base
                .as_ref()
                .map(|b| b.join("byeadhd").join("byeadhd-data.json"));
            let store = Store::load(primary, electron_src, byeadhd_src);
            let open = store.data.settings.open_at_login;
            handle.manage(AppState {
                store: Mutex::new(store),
                schema_cache: Mutex::new(None),
                notion_busy: Mutex::new(false),
                siren: Mutex::new(SirenState::default()),
                mini_postits: Mutex::new(HashMap::new()),
            });
            apply_autostart(&handle, open);
            create_tray(&handle)?;
            create_postit_window(&handle);
            sync_postit_mini_windows(&handle);
            show_dashboard(&handle);
            spawn_tick(handle.clone());
            spawn_notion_poll(handle.clone());
            spawn_update_poll(handle.clone());
            spawn_postit_cursor_poll(handle.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building MaxAlert")
        .run(|_app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                if !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        parse_update_manifest, sibling_url, update_url_allowed, version_is_newer, UpdateManifest,
    };

    #[test]
    fn update_url_scheme_restriction() {
        assert!(update_url_allowed("https://github.com/x/latest.yml"));
        assert!(update_url_allowed("http://127.0.0.1:8899/latest.yml"));
        assert!(update_url_allowed("http://localhost:8899/latest.yml"));
        assert!(!update_url_allowed("http://100.95.243.74:8899/latest.yml"));
        assert!(!update_url_allowed("ftp://example.com/latest.yml"));
    }

    #[test]
    fn update_manifest_parses_electron_builder_latest_yml() {
        // 파트너 릴리스 채널의 실제 latest.yml 형태 (v0.1.17 실물 기준)
        let yml = "version: 0.1.17\nfiles:\n  - url: MaxAlert-Setup-0.1.17.exe\n    sha512: nested==\n    size: 96034228\npath: MaxAlert-Setup-0.1.17.exe\nsha512: topLevel==\nreleaseDate: '2026-07-15T03:38:44.350Z'\n";
        assert_eq!(
            parse_update_manifest(yml),
            Some(UpdateManifest {
                version: "0.1.17".into(),
                path: "MaxAlert-Setup-0.1.17.exe".into(),
                sha512_b64: "topLevel==".into(),
            })
        );
    }

    #[test]
    fn update_manifest_rejects_incomplete_yml() {
        assert_eq!(parse_update_manifest("version: 0.2.2\n"), None);
        assert_eq!(parse_update_manifest(""), None);
    }

    #[test]
    fn sibling_url_joins_manifest_directory() {
        assert_eq!(
            sibling_url(
                "https://github.com/o/r/releases/latest/download/latest.yml",
                "MaxAlert-Setup-0.2.2.exe"
            )
            .as_deref(),
            Some("https://github.com/o/r/releases/latest/download/MaxAlert-Setup-0.2.2.exe")
        );
        assert_eq!(sibling_url("no-slash", "a.exe"), None);
    }

    #[test]
    fn version_comparison_detects_upgrade() {
        assert_eq!(version_is_newer("0.2.2", "0.2.1"), Some(true));
        assert_eq!(version_is_newer("1.0.0", "0.99.99"), Some(true));
    }

    #[test]
    fn version_comparison_rejects_equal_version() {
        assert_eq!(version_is_newer("0.2.1", "0.2.1"), Some(false));
    }

    #[test]
    fn version_comparison_rejects_downgrade() {
        assert_eq!(version_is_newer("0.2.0", "0.2.1"), Some(false));
        assert_eq!(version_is_newer("0.9.9", "1.0.0"), Some(false));
    }

    #[test]
    fn version_comparison_ignores_parse_failures() {
        assert_eq!(version_is_newer("0.2", "0.2.1"), None);
        assert_eq!(version_is_newer("v0.2.2", "0.2.1"), None);
        assert_eq!(version_is_newer("0.2.2.1", "0.2.1"), None);
        assert_eq!(version_is_newer("0.two.2", "0.2.1"), None);
        assert_eq!(version_is_newer("0.2.2", "current"), None);
    }
}

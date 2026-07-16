// MaxAlert — Tauri v2 백엔드.
// Electron main.js 의 창 관리 / 트레이 / 스케줄러 / IPC 를 Tauri 로 이식한다.
mod logic;
mod notion;
mod store;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{ManagerExt as _, MacosLauncher};
use tauri_plugin_clipboard_manager::ClipboardExt;

use logic::{now_ms, today_str, tomorrow_str, SIREN_LEAD_MS};
use store::{Data, PostitPos, Settings, Store, Todo};

const COLORS: [&str; 6] = ["yellow", "pink", "blue", "green", "purple", "orange"];
const AUTOPLAY_RESUME_JS: &str = "window.__maxalertResumeAudio && window.__maxalertResumeAudio();";

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

// macOS Tauri 는 ignore_cursor_events 상태에서 Electron 의 forward 처럼 mousemove 를
// 웹뷰에 전달해주지 않는다. 그래서 클릭스루 해제 판정에 쓸 커서 좌표를 Rust 가
// 전역 폴링해 postit 웹뷰로 밀어준다. 판정(.interactive 위인지)은 api-tauri.js
// 한 곳에서만 한다 — Rust 는 좌표 공급과 토글 실행만 담당.
fn spawn_postit_cursor_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(80));
        let mut was_inside = false;
        let mut last_sent: Option<(i32, i32)> = None;
        loop {
            interval.tick().await;
            let Some(win) = app.get_webview_window("postit") else {
                continue;
            };
            if !win.is_visible().unwrap_or(false) {
                continue;
            }
            let Ok(cursor) = app.cursor_position() else {
                continue;
            };
            let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
                continue;
            };
            let inside = cursor.x >= pos.x as f64
                && cursor.y >= pos.y as f64
                && cursor.x < pos.x as f64 + size.width as f64
                && cursor.y < pos.y as f64 + size.height as f64;
            if inside {
                let scale = win.scale_factor().unwrap_or(1.0);
                let lx = ((cursor.x - pos.x as f64) / scale).round() as i32;
                let ly = ((cursor.y - pos.y as f64) / scale).round() as i32;
                if last_sent != Some((lx, ly)) {
                    last_sent = Some((lx, ly));
                    let _ = app.emit_to("postit", "postit:cursor", json!({ "x": lx, "y": ly }));
                }
                was_inside = true;
            } else if was_inside {
                was_inside = false;
                last_sent = None;
                let _ = app.emit_to("postit", "postit:cursor", json!({ "x": -1, "y": -1 }));
            }
        }
    });
}

fn toggle_postit(app: &AppHandle) {
    match app.get_webview_window("postit") {
        Some(win) => {
            let visible = win.is_visible().unwrap_or(true);
            if visible {
                let _ = win.hide();
            } else {
                let _ = win.show();
            }
        }
        None => create_postit_window(app),
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
    let monitors = app.available_monitors().unwrap_or_default();
    let primary_pos = app.primary_monitor().ok().flatten().map(|m| *m.position());
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
        let pos = *m.position();
        let size = *m.size();
        let app2 = app.clone();
        let label2 = label.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(win) =
                WebviewWindowBuilder::new(&app2, label2, WebviewUrl::App(url.into()))
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
                let _ = win.set_position(PhysicalPosition::new(pos.x, pos.y));
                let _ = win.set_size(PhysicalSize::new(size.width, size.height));
                let _ = win.show();
                let _ = win.set_focus();
                let _ = win.set_always_on_top(true);
                // 오디오 자동재생 폴백 (JS AudioContext resume 훅 호출)
                let _ = win.eval(AUTOPLAY_RESUME_JS);
            }
        });
        labels.push(label);
    }
    {
        let mut s = state.siren.lock().unwrap();
        s.labels = labels;
    }
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
        let idx = match store.data.todos.iter().position(|t| t.id == id) {
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
        let idx = match store.data.todos.iter().position(|t| t.id == id) {
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
        if let Some(i) = store.data.todos.iter().position(|t| t.id == id) {
            store.data.todos.remove(i);
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
        let idx = match store.data.todos.iter().position(|t| t.id == id) {
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
        logic::add_points(&mut store.data, -3, &format!("⏳ 미루기: {}", title));
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
        let idx = match store.data.todos.iter().position(|t| t.id == id) {
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
fn settings_get(state: State<AppState>) -> Settings {
    state.store.lock().unwrap().data.settings.clone()
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
                return json!({ "ok": false, "error": format!("포인트 부족 (⚡{} 필요)", cost) });
            }
            logic::add_points(&mut store.data, -cost, &format!("🛍️ 테마 구입: {}", id));
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
fn postit_mouse(app: AppHandle, ignore: bool) -> bool {
    if let Some(win) = app.get_webview_window("postit") {
        let _ = win.set_ignore_cursor_events(ignore);
    }
    true
}

#[tauri::command]
fn postit_drag_start(app: AppHandle) -> bool {
    // 지은 v0.2.1 확정 해법: 네이티브 start_dragging() 1회 호출 (커서 추적 타이머 금지)
    if let Some(win) = app.get_webview_window("postit") {
        let _ = win.start_dragging();
    }
    true
}

#[tauri::command]
fn postit_drag_end() -> bool {
    true
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
            });
            apply_autostart(&handle, open);
            create_tray(&handle)?;
            create_postit_window(&handle);
            show_dashboard(&handle);
            spawn_tick(handle.clone());
            spawn_notion_poll(handle.clone());
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

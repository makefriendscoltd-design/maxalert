# MaxAlert Tauri v2 스캐폴드 결과 (2026-07-07)

브랜치: `feature/tauri-unified` (base `master` 0e75923)
목표: Electron 앱(main.js 742줄)을 Tauri v2 스캐폴드로 전환. 렌더러(renderer/*.html)는 무수정 재사용, window.api 호환 래퍼로 연결.
Phase: 1 (스캐폴드 + 컴파일/로직 검증). GUI 실기 검증은 사용자 몫.

## 1. 구현 내역

### 추가된 파일
- `src-tauri/Cargo.toml` — tauri v2(tray-icon, image-png, macos-private-api), single-instance / clipboard-manager / autostart 플러그인, serde, reqwest(json, rustls-tls), tokio, chrono.
- `src-tauri/tauri.conf.json` — productName MaxAlert, identifier `com.makefriends.maxalert`, frontendDist `../renderer`, devUrl 없음(정적), windows 배열 비움(창은 코드 생성), withGlobalTauri true, macOSPrivateApi true(투명창).
- `src-tauri/build.rs`, `src-tauri/capabilities/default.json`(`core:default`, windows `["*"]`).
- `src-tauri/icons/*` — `tauri icon`으로 RGBA 아이콘 세트 생성(원본 build/icon.png 512x512 RGB→RGBA).
- `src-tauri/src/store.rs` — lib/store.js와 동일 스키마(serde `rename_all="camelCase"`). Electron/byeadhd 마이그레이션, 30일 정리. `serde_json::to_string_pretty`로 2-space 저장(JSON.stringify(...,2) 호환).
- `src-tauri/src/logic.rs` — addPoints/levelInfo/checkBadges/maybeReward/todayPoints/profilePayload/buildDailyReport/sortTodos/pickColor/sirenEligible 이식 + 단위 테스트 5개.
- `src-tauri/src/notion.rs` — getSchema/listUsers/queryDay/setDone reqwest 이식(스키마 자동감지 동일).
- `src-tauri/src/lib.rs` — IPC 커맨드 20개 + open_siren/close_siren, 창 4종 생성, 트레이, 1초 틱/60초 노션폴 타이머, 700ms 최전면 강제 타이머.
- `src-tauri/src/main.rs` — `maxalert_lib::run()` 호출(릴리스에서 windows_subsystem).
- `renderer/api-tauri.js` — window.api 23개 메서드 호환 래퍼 + 오디오 자동재생 폴백 훅.

### 수정된 파일 (최소)
- `renderer/{postit,dashboard,siren,reward}.html` — 각 파일 기존 `<script>` **앞에 `<script src="api-tauri.js"></script>` 1줄만** 추가. 기존 로직 무수정.
- `package.json` — `@tauri-apps/cli`(dev)·`@tauri-apps/api`(dep)·tauri 스크립트 추가. 기존 electron 필드 유지(병행).
- `.gitignore` — `src-tauri/target/`, `src-tauri/gen/` 추가.

Electron 파일(main.js, preload.js, lib/)은 **삭제·수정하지 않음**.

### 로직 배치 결정 (스펙의 애매점 처리)
- `todos` 이벤트/`todos_list` 응답 페이로드는 **main.js `pushTodos`와 동일 구조**(`{todos, now, streak, profile, theme}`)로 통일. 스펙 괄호("raw만 전달")보다 **렌더러 무수정 원칙**을 우선 — postit/dashboard가 `data.profile`·`data.theme`를 직접 소비하므로 profilePayload를 Rust로 이식해 함께 전달. (렌더러 코드를 건드리지 않는 유일한 방법)
- Tauri invoke 인자 키는 전부 단어 1개(payload/id/patch/minutes/ignore/todo)로 통일 — camelCase↔snake_case 자동변환 여지를 원천 차단.

## 2. 검증 결과 (실제 실행)

| 검증 | 명령 | 결과 |
|---|---|---|
| 컴파일 | `cargo check` | PASS (0 error, 0 warning) |
| 단위 테스트 | `cargo test` | PASS — `5 passed; 0 failed` |
| 바이너리 빌드 | `cargo build` | PASS — `target/debug/maxalert` (43MB) 생성 |
| 래퍼 구문 | `node --check renderer/api-tauri.js` | PASS |

테스트 5종: level_boundaries(레벨 경계), badges_awarded_once_and_by_condition(뱃지 판정·중복방지), sort_order_done_last_then_due_then_created(정렬), pick_color_is_deterministic_and_in_palette(색상 해시), siren_eligibility_respects_ack_and_lead(사이렌 판정).

빌드 중 잡은 이슈 3건(모두 해결): macOSPrivateApi 사용 시 Cargo `macos-private-api` 피처 필요 / 트레이·번들 아이콘 RGBA 필수(RGB 원본→tauri icon 재생성) / `include_image!` 경로는 crate manifest 기준(`icons/icon.png`).

## 3. 미해결·리스크 (GUI 세션 필요 — 코드 경로만 준비)

### 리스크 3종 (스펙 지정) — 코드상 준비 상태
1. **오디오 자동재생 (사이렌 소리)** — 두 갈래 준비 완료, 실소리 미검증.
   - (a) `tauri.conf.json`엔 autoplay 직접 스위치가 없음(Electron `autoplay-policy`에 대응하는 config 부재). 대신 `api-tauri.js`가 siren 스크립트보다 먼저 로드되어 `AudioContext`/`webkitAudioContext`를 패치 — 생성 즉시 `resume()` + `window.__maxalertResumeAudio()` 훅 등록.
   - (b) Rust가 사이렌 창 로드 직후 `win.eval(window.__maxalertResumeAudio())` 폴백 호출.
   - 사이렌 소리는 오디오 파일이 아니라 **Web Audio 합성**(siren.html)이라, 관건은 suspended AudioContext의 resume. 위 패치로 대응하나 WKWebView 정책상 제스처 없는 resume이 막히면 (a)(b) 모두 실패 가능 → 그 경우 Rust rodio 분리(마이그레이션 플랜 리스크1 대안) 필요.
2. **클릭스루 forward** — postit 창 `set_ignore_cursor_events(true)` 초기값 적용, `postit_mouse(ignore)` 커맨드로 hover 토글. Electron `{forward:true}` 상당 동작은 플랫폼별이라 실기 확인 필요.
3. **사이렌 최전면 강제** — 디스플레이별 프레임리스 전체-bounds 창(siren-{i}, kiosk 아님) + `always_on_top` + 700ms `set_always_on_top`+`set_focus` 반복 타이머. macOS "도망 못 가는" 수준은 OS 제약이라 실기 확인 필요(플랜 리스크2와 동일).

### 추가로 발견한 리스크/한계
- **에셋 경로**: siren.html이 `../assets/chick.mp4`를 참조하는데 frontendDist가 `../renderer`라 assets/는 임베드 밖 → Tauri asset 프로토콜이 서빙 못함. 병아리 영상은 siren.html의 error 핸들러가 숨김 처리(크래시 없음)하지만 **표시 안 됨**. Phase 2에서 frontendDist 조정 또는 assets를 renderer로 이동/커스텀 프로토콜 필요.
- **드래그**: postit 드래그는 `start_dragging()` 1회 호출(지은 v0.2.1 확정 해법, 커서추적 타이머 금지)로 이식. dragEnd는 no-op.
- **postitPos 좌표계**: Tauri는 물리 픽셀(Moved 이벤트/set_position)로 저장·복원. Electron은 논리 좌표로 저장 → HiDPI에서 상호 해석이 어긋날 수 있음(위치만 cosmetic, 드래그 시 자가보정). 스키마(`{x,y}`)는 호환.
- **postit 높이**: primary monitor 전체 size 기준(work_area 미사용) → dock/메뉴바와 겹칠 수 있음. Phase 2 조정.
- **노션 로컬→노션 done 반영**: async 제약상 락 해제 후 `set_done` 호출. `notion_done`을 낙관적으로 먼저 갱신하므로 API 실패 시 재동기화까지 불일치 가능(next 60초 폴에서 교정).
- **자동 업데이트**: 미포함(플랜 Phase 3). Electron 잔존 사용자 수동 재설치 안내 필요.
- **데이터 경로**: 신규 파일은 `app_data_dir()`(=`~/Library/Application Support/com.makefriends.maxalert/`). Electron은 `.../maxalert/`. 최초 실행 시 (a)Electron (b)byeadhd 순으로 복사 마이그레이션. **Electron↔Tauri 동시 운영 시 두 파일이 분리**됨(마이그레이션은 1회 복사) — 병행 트랙에서 데이터 실시간 공유는 안 됨(스펙의 "양방향 읽기 가능"은 스키마 호환을 뜻하며, 같은 파일을 동시에 보는 것은 아님).

## 4. 다음 단계 제안 (Phase 2 진입 전)
1. **GUI 실기 검증**: `npx tauri dev`로 포스트잇/대시보드 표시 + 리스크 3종(오디오·클릭스루·최전면) 실확인 → CEO 게이트.
2. 에셋 경로 해결(frontendDist 또는 커스텀 프로토콜)로 병아리 영상 복구.
3. postit work_area 높이 + postitPos 논리좌표 정합.
4. 오디오 실패 시 rodio 분리 경로 결정.
5. Electron↔Tauri 병행 기간의 데이터 파일 단일화 정책(경로 통일 or 심볼릭) 결정.

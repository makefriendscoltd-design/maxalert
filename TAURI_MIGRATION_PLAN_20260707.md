# MaxAlert Tauri 전환 계획 (2026-07-07)

결정(CEO, 2026-07-07): Windows 포함 전체 Tauri 단일화. Electron 이중 트랙 없음. 사용자 거의 없어 마이그레이션 부담 최소 시점.
참조 선례: 지은(aimax-viseo) Tauri macOS 포팅 — `AIMAX-AI-Staff-Management/handoffs/2026-07-01-jieun-tauri-mac-port/TAURI_MIGRATION_PLAN_20260701.md`

## 1. 현재 앱 인벤토리 (Electron v0.1.3 실측)

코드 규모: main.js 742줄, preload.js 25줄, lib/ 146줄(notion.js·store.js), renderer/ HTML 4개 1,545줄. **렌더러는 vanilla JS** — 프레임워크 없음, window.api 호출부만 바꾸면 거의 그대로 재사용된다.

### 창 4종
| 창 | Electron 구성 | 특이점 |
|---|---|---|
| 대시보드 | 일반 창 540x780 | todo CRUD·설정·테마 상점·프로필 |
| 포스트잇 위젯 | frameless, transparent, focusable:false, skipTaskbar, alwaysOnTop(floating), 클릭스루(setIgnoreMouseEvents+forward) | 메인 프로세스 커서 추적 드래그(v0.1.2에서 교체 — 지은과 동일한 문제를 이미 겪음), hover 시 클릭스루 해제 |
| 사이렌 | 디스플레이별 kiosk 전체화면 최상위 + 700ms 포커스 강제 타이머 | 주 모니터만 소리, 병아리 크로마키 비디오(canvas), 닫기 불가 |
| 보상 | 주 모니터 전체 투명 오버레이, 30초 자동 닫힘 | |

### 시스템 통합
| 기능 | Electron API | 비고 |
|---|---|---|
| 트레이 | Tray + 동적 비트맵 아이콘 + 컨텍스트 메뉴 | 클릭→대시보드 |
| 상주 | window-all-closed 무시, 트레이 상주 | |
| 단일 인스턴스 | requestSingleInstanceLock | 2번째 실행→대시보드 |
| 자동 시작 | setLoginItemSettings | 설정 토글 |
| 자동 업데이트 | electron-updater → GitHub Releases(maxalert-releases) | 1시간 주기 |
| 클립보드 | clipboard.writeText | 일일 보고 복사 |
| 저장소 | userData/maxalert-data.json 직접 R/W | 구 byeadhd 마이그레이션 포함 |
| 오디오 자동재생 | autoplay-policy 스위치 | 사이렌 소리에 필수 |

### 비즈니스 로직 (main 프로세스 상주)
- 1초 틱 스케줄러: 사이렌 대상 판정(3분 전), 15초마다 todo push
- 포인트/레벨/뱃지/스트릭/보상 판정
- 노션 동기화(60초 폴링, 스키마 자동감지, 양방향 done, 담당자 필터) — lib/notion.js는 순수 fetch 89줄
- IPC 핸들러 21개 (preload.js에 전체 목록)

## 2. Tauri 대응표

| Electron | Tauri 대응 | 난이도 | 지은 선례 |
|---|---|---|---|
| BrowserWindow(대시보드) | WebviewWindow | 하 | 동일 패턴 |
| 포스트잇(투명·클릭스루·최상위) | decorations:false, transparent, always_on_top, skip_taskbar, focusable:false + set_ignore_cursor_events | 중 | 캐릭터 창과 동일 구성 검증됨. macOS는 accept_first_mouse 추가 |
| 포스트잇 드래그(커서 추적) | **start_dragging() 네이티브 1회 호출** | 하 | v0.2.1에서 확정된 해법 그대로. 혼합 DPI 검증 완료 |
| 사이렌 멀티모니터(kiosk) | **디스플레이별 창 생성** (capture-overlay-{i} 패턴) + set_always_on_top + 주기적 set_focus | 중 | macOS 'Spaces 분리'에서 창은 한 디스플레이에만 붙음 — 지은에서 해결한 핵심 이슈. kiosk 대신 프레임리스+전체 bounds 권장 |
| 트레이 | tray-icon (Tauri v2 내장) | 하 | jieun-tray 좌클릭 토글·우클릭 메뉴 검증됨. 동적 비트맵 아이콘은 정적 PNG 교체 |
| 단일 인스턴스 | tauri-plugin-single-instance | 하 | |
| 자동 시작 | tauri-plugin-autostart | 하 | |
| 자동 업데이트 | tauri-plugin-updater + GitHub Releases | 중 | electron-updater와 메타데이터 다름(latest.json + 서명키). 릴리스 파이프라인 재작성. Electron 구버전 사용자는 자동 전환 불가 → 수동 재설치 안내(사용자 거의 없어 허용) |
| 클립보드 | tauri clipboard-manager 플러그인 | 하 | |
| JSON 저장소 | Rust fs 커맨드(store 로직 이식) 또는 tauri-plugin-store | 하 | 57줄 |
| 노션 fetch | **Rust reqwest 커맨드로 이동 필수** — Notion API는 CORS 미허용이라 웹뷰에서 직접 fetch 불가 | 중 | 89줄, 엔드포인트 4개 |
| autoplay | macOS WKWebView·Windows WebView2 각각 autoplay 정책 설정/우회 확인 | **리스크** | 미검증 — Phase 1에서 최우선 확인 |
| IPC 21개 | invoke 커맨드 + 이벤트(emit) | 중 | preload가 이미 얇은 어댑터라 window.api 시그니처 유지한 호환 래퍼로 렌더러 무수정 가능 |

### 로직 배치 결정 (권장: 하이브리드)
- **Rust로 이동**: 창 관리, 트레이, 1초 틱+사이렌 판정 타이머(웹뷰 백그라운드 스로틀링 회피 — 크롬 occlusion rAF freeze 선례), 파일 저장소, 노션 HTTP, 클립보드.
- **JS 유지(렌더러)**: 포인트/레벨/뱃지/보상 판정, 정렬/표시 로직 — 대시보드·포스트잇이 이미 소비하는 데이터라 이동 비용만 생김. Rust는 상태를 저장·중계만.
- 근거: 사용자 거의 없음 + 빠른 검증 우선. 정석(전부 Rust)보다 재사용 극대화. 문제가 생기는 조각만 Rust로 승격.

## 3. 리스크 (우선순위순)

1. **오디오 자동재생**: 사이렌은 사용자 제스처 없이 소리가 나야 한다. WKWebView/WebView2의 autoplay 정책이 Electron 스위치와 다름. Phase 1 스캐폴드에서 가장 먼저 실검증. 실패 시 대안: Rust 측 오디오 재생(rodio crate)으로 소리만 분리.
2. **사이렌 강제성**: Electron kiosk+closable:false+포커스 강제 700ms의 재현 수준. macOS에서 앱 강제 최전면은 OS 제약이 있음 — "도망 못 가는" 수준을 실기로 확인하고 필요하면 NSWindow 레벨 조정.
3. **클릭스루 forward**: setIgnoreMouseEvents(true, {forward:true})의 hover 감지 동작이 Tauri에서 플랫폼별 상이. 포스트잇 UX 핵심이라 Phase 1에서 확인.
4. 업데이트 채널 전환: Electron 사용자 잔존분은 자동 업데이트로 못 넘어옴 — 수동 안내 1회.

## 4. Phase 계획

### Phase 1 — 스캐폴드 + 리스크 3종 실검증 (이번 라운드)
- `src-tauri/` 추가, 창 4종 구성 이식, window.api 호환 래퍼, 트레이.
- 검증: `cargo check` PASS, `tauri dev`로 포스트잇·대시보드 표시, **오디오 자동재생·클릭스루·사이렌 최전면 3종 실기 확인 보고**.
- 산출: 브랜치 `feature/tauri-unified`, 리스크 판정 결과 → CEO 게이트.

### Phase 2 — 기능 완성 (맥 MVP)
- 범위: todo CRUD, 사이렌(멀티모니터+병아리), 포스트잇 전체 UX, 노션 동기화(Rust), 일일 보고 복사, 저장소(+byeadhd/Electron 데이터 마이그레이션), 포인트/뱃지.
- 후순위(있으면 유지, 막히면 Phase 3): 테마 상점, 보상 오버레이, 자동 시작.
- 검증: 맥 실기 e2e (사이렌 실울림 포함) → CEO 실기 확인 게이트.

### Phase 3 — Windows Tauri 빌드 + 릴리스 전환
- 같은 코드베이스 Windows 빌드(NSIS/MSI), tauri-plugin-updater 릴리스 파이프라인(maxalert-releases에 latest.json+서명), Electron 잔존 사용자 재설치 안내문.
- 검증: Windows 실기 e2e(윈도우 머신 Claude 핸드오프) → 배포 게이트.

### 배포 순서
맥 알파(수동 DMG) → 맥 안정화 → Windows Tauri → 업데이트 채널 정착. AIMAX 직원 카탈로그 등록은 별도 라운드(제품명·가격·화이트리스트 CEO 결정 필요).

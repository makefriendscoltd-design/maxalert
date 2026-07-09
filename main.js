const { app, BrowserWindow, ipcMain, screen, Tray, Menu, nativeImage, clipboard } = require('electron')
const path = require('path')
const fs = require('fs')
const { autoUpdater } = require('electron-updater')
const Store = require('./lib/store')
const notion = require('./lib/notion')

// 사이렌 오디오가 사용자 제스처 없이 재생되도록 허용
app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required')

const SIREN_LEAD_MS = 3 * 60 * 1000 // 일정 3분 전부터 사이렌
const PRELOAD = path.join(__dirname, 'preload.js')

let store
let tray = null
let dashboardWin = null
let postitWin = null
let rewardWin = null
let sirenWins = []
let sirenTodoId = null
let focusTimer = null
let tickCount = 0
let notionBusy = false
let schemaCache = null // { key, schema }
let quitting = false

// ---------- 포인트 / 레벨 / 뱃지 ----------
// 알 → (금이 가며 깨짐) → 병아리 → 닭 으로 자라는 진화 레벨. 포인트(min) 구간은 유지.
const LEVELS = [
  { min: 0, title: '잠꾸러기 알', icon: '🥚', stage: 'egg' },        // 멀쩡한 알
  { min: 100, title: '꿈틀대는 알', icon: '🥚', stage: 'egg-crack' }, // 금이 가기 시작
  { min: 250, title: '부화 시작', icon: '🐣', stage: 'hatch' },       // 껍질 깨고 얼굴 빼꼼
  { min: 500, title: '껍질 깨는 중', icon: '🐣', stage: 'hatch2' },   // 거의 다 깨짐
  { min: 900, title: '갓난 병아리', icon: '🐤', stage: 'chick' },
  { min: 1400, title: '씩씩한 병아리', icon: '🐥', stage: 'chick2' },
  { min: 2000, title: '풋내기 닭', icon: '🐔', stage: 'hen' },
  { min: 2800, title: '당당한 닭', icon: '🐔', stage: 'hen2' },
  { min: 3800, title: '우두머리 장닭', icon: '🐓', stage: 'rooster' },
  { min: 5000, title: '전설의 갓닭', icon: '🐓', stage: 'legend' }
]

const BADGES = [
  { id: 'first-done', name: '첫 걸음', icon: '🌱', desc: '첫 할 일 완료', test: c => c.stats.totalDone >= 1 },
  { id: 'early-bird', name: '얼리버드', icon: '🌅', desc: '오전 9시 전에 할 일 완료', test: c => c.earlyDone },
  { id: 'perfect-day', name: '퍼펙트 데이', icon: '💯', desc: '하루의 할 일 전부 완료', test: c => c.allDone },
  { id: 'no-snooze', name: '정면돌파', icon: '🛡️', desc: '미루기 없이 하루 클리어', test: c => c.noSnoozeDay },
  { id: 'streak-3', name: '작심삼일 극복', icon: '🔥', desc: '3일 연속 전체 완료', test: c => c.streak.count >= 3 },
  { id: 'streak-7', name: '일주일의 기적', icon: '🌈', desc: '7일 연속 전체 완료', test: c => c.streak.count >= 7 },
  { id: 'siren-slayer', name: '사이렌 슬레이어', icon: '🚨', desc: '사이렌이 울리는 중에 완료 5회', test: c => c.stats.sirenSaves >= 5 },
  { id: 'centurion', name: '백전노장', icon: '⚔️', desc: '누적 100개 완료', test: c => c.stats.totalDone >= 100 }
]

// 포인트 상점: 포스트잇 테마 (⚡ 가격)
const THEME_PRICES = { classic: 0, neon: 150, kraft: 200, midnight: 300 }

function addPoints(delta, reason) {
  const p = store.data.points
  p.total = Math.max(0, p.total + delta)
  p.ledger.unshift({ at: Date.now(), delta, reason })
  if (p.ledger.length > 200) p.ledger.length = 200
}

function levelInfo(total) {
  let idx = 0
  for (let i = 0; i < LEVELS.length; i++) if (total >= LEVELS[i].min) idx = i
  const cur = LEVELS[idx]
  const next = LEVELS[idx + 1] || null
  return { n: idx + 1, title: cur.title, icon: cur.icon, min: cur.min, next: next ? next.min : null }
}

function ownBadge(id) { return store.data.badges.some(b => b.id === id) }

function checkBadges() {
  const todos = store.todosOn(todayStr())
  const allDone = todos.length > 0 && todos.every(t => t.done)
  const ctx = {
    stats: store.data.stats,
    streak: store.data.streak,
    allDone,
    noSnoozeDay: allDone && todos.every(t => !(t.postpones > 0)),
    earlyDone: todos.some(t => t.done && t.doneAt && new Date(t.doneAt).getHours() < 9)
  }
  for (const b of BADGES) {
    if (!ownBadge(b.id) && b.test(ctx)) {
      store.data.badges.push({ id: b.id, at: Date.now() })
      addPoints(25, `🏅 뱃지 획득: ${b.name}`)
    }
  }
}

function todayPoints() {
  return store.data.points.ledger
    .filter(e => dateStrOf(new Date(e.at)) === todayStr())
    .reduce((s, e) => s + e.delta, 0)
}

function profilePayload() {
  const total = store.data.points.total
  return {
    total,
    todayPoints: todayPoints(),
    level: levelInfo(total),
    badges: BADGES.map(b => ({
      id: b.id, name: b.name, icon: b.icon, desc: b.desc,
      at: (store.data.badges.find(x => x.id === b.id) || {}).at || null
    })),
    stats: store.data.stats,
    streak: store.data.streak,
    ledger: store.data.points.ledger.slice(0, 12)
  }
}

// ---------- 날짜 헬퍼 ----------
function dateStrOf(d) {
  const p = n => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}
function todayStr() { return dateStrOf(new Date()) }
function tomorrowStr() { return dateStrOf(new Date(Date.now() + 86400000)) }

// ---------- 앱 시작 ----------
const gotLock = app.requestSingleInstanceLock()
if (!gotLock) {
  app.quit()
} else {
  app.on('second-instance', () => showDashboard())
  app.whenReady().then(init)
}

app.on('window-all-closed', () => { /* 트레이 상주 — 종료하지 않음 */ })
app.on('before-quit', () => {
  quitting = true
  closeSiren()
})

function init() {
  const dataFile = path.join(app.getPath('userData'), 'maxalert-data.json')
  // 구 앱 이름(byeadhd) 시절 데이터 마이그레이션
  if (!fs.existsSync(dataFile)) {
    const oldFile = path.join(path.dirname(app.getPath('userData')), 'byeadhd', 'byeadhd-data.json')
    try {
      if (fs.existsSync(oldFile)) {
        fs.mkdirSync(path.dirname(dataFile), { recursive: true })
        fs.copyFileSync(oldFile, dataFile)
      }
    } catch { /* 마이그레이션 실패 시 새로 시작 */ }
  }
  store = new Store(dataFile)
  registerIpc()
  createTray()
  createPostitWindow()
  showDashboard()
  setInterval(tick, 1000)
  setInterval(() => syncNotion().catch(() => {}), 60000)
  syncNotion().catch(() => {})
  applyLoginItem()
  setupAutoUpdate()
}

// ---------- 자동 업데이트 (GitHub Releases: maxalert-releases) ----------
function setupAutoUpdate() {
  if (!app.isPackaged) return // 개발 모드에서는 비활성
  autoUpdater.autoDownload = true
  autoUpdater.autoInstallOnAppQuit = true // 다운로드 후 앱 종료 시 자동 설치
  const check = () => autoUpdater.checkForUpdatesAndNotify().catch(() => {})
  check()
  setInterval(check, 60 * 60 * 1000) // 1시간마다 확인
}

// ---------- 트레이 ----------
function makeTrayIcon() {
  const size = 16
  const buf = Buffer.alloc(size * size * 4)
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = x - 7.5, dy = y - 7.5
      const i = (y * size + x) * 4
      if (Math.sqrt(dx * dx + dy * dy) < 7.2) {
        // BGRA — 노란 포스트잇 색
        buf[i] = 90; buf[i + 1] = 210; buf[i + 2] = 250; buf[i + 3] = 255
      }
    }
  }
  return nativeImage.createFromBitmap(buf, { width: size, height: size })
}

function createTray() {
  tray = new Tray(makeTrayIcon())
  tray.setToolTip('MaxAlert — 일정 사이렌')
  const menu = Menu.buildFromTemplate([
    { label: '투두리스트 열기', click: () => showDashboard() },
    {
      label: '포스트잇 보이기/숨기기',
      click: () => {
        if (!postitWin || postitWin.isDestroyed()) createPostitWindow()
        else if (postitWin.isVisible()) postitWin.hide()
        else postitWin.showInactive()
      }
    },
    { label: '지금 노션 동기화', click: () => syncNotion().catch(() => {}) },
    { type: 'separator' },
    { label: '종료', click: () => app.quit() }
  ])
  tray.setContextMenu(menu)
  tray.on('click', () => showDashboard())
}

// ---------- 대시보드 ----------
function showDashboard() {
  if (dashboardWin && !dashboardWin.isDestroyed()) {
    dashboardWin.show()
    dashboardWin.focus()
    return
  }
  dashboardWin = new BrowserWindow({
    width: 540, height: 780,
    title: 'MaxAlert',
    autoHideMenuBar: true,
    webPreferences: { preload: PRELOAD }
  })
  dashboardWin.loadFile('renderer/dashboard.html')
  dashboardWin.on('closed', () => { dashboardWin = null })
}

// ---------- 포스트잇 위젯 ----------
function createPostitWindow() {
  const { workArea } = screen.getPrimaryDisplay()
  const width = 340
  const saved = store.data.settings.postitPos
  postitWin = new BrowserWindow({
    x: saved ? saved.x : workArea.x + workArea.width - width,
    y: saved ? saved.y : workArea.y,
    width,
    height: workArea.height,
    frame: false,
    transparent: true,
    resizable: false,
    movable: true,
    skipTaskbar: true,
    focusable: false,
    hasShadow: false,
    alwaysOnTop: true,
    webPreferences: { preload: PRELOAD }
  })
  postitWin.setAlwaysOnTop(true, 'floating')
  postitWin.setIgnoreMouseEvents(true, { forward: true })
  postitWin.loadFile('renderer/postit.html')
  postitWin.webContents.on('did-finish-load', () => pushTodos())
  // 드래그로 옮긴 위치 기억 + 화면 중앙을 넘어가면 슬라이드 방향 반전
  let moveTimer = null
  let lastSide = null
  postitWin.on('move', () => {
    const side = postitSide()
    if (side !== lastSide) { lastSide = side; pushTodos() }
    clearTimeout(moveTimer)
    moveTimer = setTimeout(() => {
      if (!postitWin || postitWin.isDestroyed()) return
      const [x, y] = postitWin.getPosition()
      store.data.settings.postitPos = { x, y }
      store.save()
    }, 500)
  })
  postitWin.on('closed', () => { postitWin = null })
}

// ---------- 사이렌 ----------
function sirenEligible(t, now) {
  return !t.done && t.dueAt && (t.dueAt - now) <= SIREN_LEAD_MS && t.ackDue !== t.dueAt
}

function openSiren(todo) {
  sirenTodoId = todo.id
  const primaryId = screen.getPrimaryDisplay().id
  sirenWins = screen.getAllDisplays().map(d => {
    const w = new BrowserWindow({
      x: d.bounds.x, y: d.bounds.y,
      width: d.bounds.width, height: d.bounds.height,
      frame: false,
      fullscreen: true,
      kiosk: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      minimizable: false,
      closable: false,
      webPreferences: { preload: PRELOAD }
    })
    w.setAlwaysOnTop(true, 'screen-saver')
    // 주 모니터에서만 소리 재생 (다중 모니터 에코 방지)
    w.loadFile('renderer/siren.html', {
      query: {
        sound: d.id === primaryId ? '1' : '0',
        volume: String(store.data.settings.sirenVolume ?? 0.5)
      }
    })
    w.webContents.on('did-finish-load', () => {
      if (!w.isDestroyed()) w.webContents.send('siren:todo', todo)
    })
    return w
  })
  // 강제성: 다른 작업으로 못 도망가게 계속 최상위 + 포커스 유지
  focusTimer = setInterval(() => {
    sirenWins.forEach(w => {
      if (!w.isDestroyed()) {
        w.setAlwaysOnTop(true, 'screen-saver')
        w.moveTop()
        w.focus()
      }
    })
  }, 700)
}

function closeSiren() {
  if (focusTimer) { clearInterval(focusTimer); focusTimer = null }
  sirenWins.forEach(w => { if (!w.isDestroyed()) w.destroy() })
  sirenWins = []
  sirenTodoId = null
}

// ---------- 스케줄러 ----------
function tick() {
  if (quitting) return
  tickCount++
  const now = Date.now()
  const todos = store.todosOn(todayStr())

  if (sirenTodoId) {
    const cur = todos.find(t => t.id === sirenTodoId)
    if (!cur || !sirenEligible(cur, now)) {
      closeSiren()
    } else {
      sirenWins.forEach(w => {
        if (!w.isDestroyed()) w.webContents.send('siren:todo', cur)
      })
    }
  }
  if (!sirenTodoId) {
    const target = todos
      .filter(t => sirenEligible(t, now))
      .sort((a, b) => a.dueAt - b.dueAt)[0]
    if (target) openSiren(target)
  }
  // 카운트다운은 렌더러가 자체 계산 — 15초마다 안전 동기화만
  if (tickCount % 15 === 0) pushTodos()
}

// 위젯이 화면의 어느 쪽 절반에 있는지 → 포스트잇 슬라이드 방향 결정용
function postitSide() {
  if (!postitWin || postitWin.isDestroyed()) return 'right'
  const [x] = postitWin.getPosition()
  const { width } = postitWin.getBounds()
  const center = x + width / 2
  const disp = screen.getDisplayNearestPoint({ x: Math.round(center), y: 0 })
  return center < disp.workArea.x + disp.workArea.width / 2 ? 'left' : 'right'
}

function pushTodos() {
  const payload = {
    todos: store.todosOn(todayStr()).sort(sortTodos),
    now: Date.now(),
    streak: store.data.streak,
    profile: profilePayload(),
    theme: store.data.settings.postitTheme || 'classic',
    side: postitSide()
  }
  for (const w of [postitWin, dashboardWin]) {
    if (w && !w.isDestroyed()) w.webContents.send('todos', payload)
  }
}

function sortTodos(a, b) {
  if (a.done !== b.done) return a.done ? 1 : -1
  if (!a.dueAt && !b.dueAt) return a.createdAt - b.createdAt
  if (!a.dueAt) return 1
  if (!b.dueAt) return -1
  return a.dueAt - b.dueAt
}

// ---------- 보상 ----------
function maybeReward() {
  const todos = store.todosOn(todayStr())
  if (!todos.length || !todos.every(t => t.done)) return
  if (store.data.lastRewardDate === todayStr()) return
  store.data.lastRewardDate = todayStr()
  const s = store.data.streak
  const yesterday = dateStrOf(new Date(Date.now() - 86400000))
  s.count = (s.lastDate === yesterday) ? s.count + 1 : 1
  s.lastDate = todayStr()
  addPoints(50, '🎉 오늘의 할 일 전체 완료 보너스')
  checkBadges()
  store.save()
  const li = levelInfo(store.data.points.total)
  openReward({
    streak: s.count,
    today: todayPoints(),
    total: store.data.points.total,
    title: li.title,
    icon: li.icon,
    next: li.next || '',
    min: li.min
  })
}

function openReward(info) {
  if (rewardWin && !rewardWin.isDestroyed()) rewardWin.destroy()
  const d = screen.getPrimaryDisplay()
  rewardWin = new BrowserWindow({
    x: d.bounds.x, y: d.bounds.y,
    width: d.bounds.width, height: d.bounds.height,
    frame: false,
    transparent: true,
    alwaysOnTop: true,
    skipTaskbar: true,
    resizable: false,
    webPreferences: { preload: PRELOAD }
  })
  rewardWin.setAlwaysOnTop(true, 'screen-saver')
  const query = {}
  for (const [k, v] of Object.entries(info)) query[k] = String(v)
  rewardWin.loadFile('renderer/reward.html', { query })
  rewardWin.on('closed', () => { rewardWin = null })
  // 안전장치: 30초 후 자동 닫기
  setTimeout(() => { if (rewardWin && !rewardWin.isDestroyed()) rewardWin.destroy() }, 30000)
}

// ---------- 노션 동기화 ----------
async function getSchemaFor(token, dbId) {
  const key = token.slice(-8) + ':' + dbId
  if (schemaCache && schemaCache.key === key) return schemaCache.schema
  const schema = await notion.getSchema(token, dbId)
  schemaCache = { key, schema }
  return schema
}

async function syncNotion() {
  const { notionToken, notionDb } = store.data.settings
  if (!notionToken || !notionDb) return { ok: false, error: '노션 토큰/DB가 설정되지 않았습니다' }
  if (notionBusy) return { ok: false, error: '동기화 진행 중' }
  notionBusy = true
  try {
    const schema = await getSchemaFor(notionToken, notionDb)
    const assigneeId = store.data.settings.notionAssignee?.id || null
    const pages = await notion.queryDay(notionToken, notionDb, schema, todayStr(), tomorrowStr(), assigneeId)
    let added = 0
    const now = Date.now()
    for (const p of pages) {
      const existing = store.data.todos.find(t => t.notionPageId === p.id)
      if (!existing) {
        const t = {
          id: 'n' + now + Math.random().toString(36).slice(2, 6),
          title: p.title,
          color: pickColor(p.title),
          date: todayStr(),
          done: p.done,
          createdAt: now,
          dueAt: p.dueAt,
          notionPageId: p.id,
          notionDueAt: p.dueAt,
          notionDone: p.done
        }
        // 이미 지난 일정은 가져오자마자 사이렌이 울리지 않게
        if (t.dueAt && t.dueAt < now) t.ackDue = t.dueAt
        store.data.todos.push(t)
        added++
      } else {
        // 일정의 제목/날짜/시간 소스는 노션. 완료(done)는 앱 로컬 전용(노션 안 건드림).
        existing.title = p.title
        if (p.dueAt !== existing.notionDueAt) {
          existing.dueAt = p.dueAt
          existing.notionDueAt = p.dueAt
          delete existing.ackDue
        }
        // 지난 날짜에 있던 항목이 오늘 다시 잡힘 = 새 반복 occurrence → 완료 상태 초기화 후 오늘로 이동
        if (existing.date !== todayStr()) {
          existing.done = false
          existing.doneAt = null
          existing.awarded = 0
          existing.date = todayStr()
        }
      }
    }
    // 필터에 더는 안 잡히는 오늘 노션 "미완료" 일정만 제거 (담당자 재배정 등).
    // 완료된 항목은 오늘의 기록이므로 절대 지우지 않음.
    const returnedIds = new Set(pages.map(p => p.id))
    const today = todayStr()
    let removed = 0
    store.data.todos = store.data.todos.filter(t => {
      if (t.notionPageId && !t.done && t.date === today && !returnedIds.has(t.notionPageId)) {
        removed++
        return false
      }
      return true
    })
    store.save()
    pushTodos()
    return { ok: true, count: pages.length, added, removed, at: Date.now() }
  } catch (err) {
    return { ok: false, error: String(err.message || err) }
  } finally {
    notionBusy = false
  }
}

// ---------- 기타 ----------
const COLORS = ['yellow', 'pink', 'blue', 'green', 'purple', 'orange']
function pickColor(seed) {
  let h = 0
  for (const c of String(seed)) h = (h * 31 + c.charCodeAt(0)) >>> 0
  return COLORS[h % COLORS.length]
}

// 오늘 완료 기록을 MD 보고서로 생성.
// 완료 이력은 포인트 원장에서 복원 → 동기화가 todos를 삭제/리셋해도 기록이 보존됨.
function buildDailyReport() {
  const today = todayStr()
  const events = store.data.points.ledger
    .filter(e => dateStrOf(new Date(e.at)) === today)
    .slice().reverse() // 오래된→최신 순
  const done = new Map() // 제목 -> 첫 완료 시각
  for (const e of events) {
    let m = e.reason.match(/^완료(?:\(노션\))?:\s*(.+?)(?:\s*\(정시.*\))?$/)
    if (m) { if (!done.has(m[1])) done.set(m[1], e.at); continue }
    m = e.reason.match(/^완료 취소(?:\(노션\))?:\s*(.+)$/)
    if (m) done.delete(m[1])
  }
  const fmt = (ms) => {
    const d = new Date(ms)
    return String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0')
  }
  const name = store.data.settings.notionAssignee?.name
  const header = name ? `${name} | ${today} 업무 보고` : `${today} 업무 보고`
  const entries = [...done.entries()].sort((a, b) => a[1] - b[1])
  const L = []
  L.push(`## 📋 ${header}`)
  L.push('')
  L.push(`완료 **${entries.length}**건`)
  L.push('')
  L.push(entries.length ? entries.map(([t, at]) => `- \`${fmt(at)}\` ${t}`).join('\n') : '- (완료한 항목 없음)')
  return L.join('\n')
}

function applyLoginItem() {
  if (!app.isPackaged && process.platform === 'win32') return // 개발 모드에서는 등록 생략
  app.setLoginItemSettings({ openAtLogin: !!store.data.settings.openAtLogin })
}

// ---------- IPC ----------
function registerIpc() {
  ipcMain.handle('todos:list', () => ({
    todos: store.todosOn(todayStr()).sort(sortTodos),
    now: Date.now(),
    streak: store.data.streak,
    profile: profilePayload(),
    theme: store.data.settings.postitTheme || 'classic',
    side: postitSide()
  }))

  ipcMain.handle('shop:buyTheme', (_e, id) => {
    const cost = THEME_PRICES[id]
    if (cost == null) return { ok: false, error: '알 수 없는 테마' }
    const s = store.data.settings
    s.unlockedThemes = s.unlockedThemes || ['classic']
    if (!s.unlockedThemes.includes(id)) {
      if (store.data.points.total < cost) {
        return { ok: false, error: `포인트 부족 (⚡${cost} 필요)` }
      }
      addPoints(-cost, `🛍️ 테마 구입: ${id}`)
      s.unlockedThemes.push(id)
    }
    s.postitTheme = id // 구입 즉시 적용
    store.save()
    pushTodos()
    return { ok: true, settings: s }
  })

  ipcMain.handle('todos:add', (_e, { title, time, color }) => {
    if (!title || !title.trim()) return null
    const t = {
      id: 't' + Date.now() + Math.random().toString(36).slice(2, 6),
      title: title.trim(),
      color: COLORS.includes(color) ? color : 'yellow',
      date: todayStr(),
      done: false,
      createdAt: Date.now(),
      dueAt: null
    }
    if (time) {
      const [h, m] = time.split(':').map(Number)
      if (!isNaN(h) && !isNaN(m)) {
        const d = new Date()
        d.setHours(h, m, 0, 0)
        t.dueAt = d.getTime()
        // 이미 지난 시간으로 등록하면 즉시 사이렌은 울리지 않음
        if (t.dueAt < Date.now()) t.ackDue = t.dueAt
      }
    }
    store.data.todos.push(t)
    store.save()
    pushTodos()
    return t
  })

  ipcMain.handle('todos:update', (_e, id, patch) => {
    const t = store.find(id)
    if (!t) return null
    if (typeof patch.title === 'string' && patch.title.trim()) t.title = patch.title.trim()
    if (COLORS.includes(patch.color)) t.color = patch.color
    if ('time' in patch) {
      if (patch.time) {
        const [h, m] = patch.time.split(':').map(Number)
        if (!isNaN(h) && !isNaN(m)) {
          const d = new Date()
          d.setHours(h, m, 0, 0)
          t.dueAt = d.getTime()
          if (t.dueAt < Date.now()) t.ackDue = t.dueAt
          else delete t.ackDue
        }
      } else {
        t.dueAt = null
        delete t.ackDue
      }
    }
    store.save()
    pushTodos()
    return t
  })

  ipcMain.handle('todos:toggle', (_e, id) => {
    const t = store.find(id)
    if (!t) return null
    const now = Date.now()
    t.done = !t.done
    if (t.done) {
      t.doneAt = now
      let pts = 10
      let reason = `완료: ${t.title}`
      if (t.dueAt && now <= t.dueAt) {
        pts += 10
        reason += ' (정시 +10)'
        if (t.dueAt - now <= SIREN_LEAD_MS) store.data.stats.sirenSaves++
      }
      t.awarded = pts
      addPoints(pts, reason)
      store.data.stats.totalDone++
      checkBadges()
      if (sirenTodoId === id) closeSiren()
    } else {
      t.doneAt = null
      addPoints(-(t.awarded || 10), `완료 취소: ${t.title}`)
      t.awarded = 0
      store.data.stats.totalDone = Math.max(0, store.data.stats.totalDone - 1)
    }
    store.save()
    pushTodos()
    if (t.done) maybeReward()
    return t
  })

  ipcMain.handle('todos:delete', (_e, id) => {
    const i = store.data.todos.findIndex(t => t.id === id)
    if (i >= 0) store.data.todos.splice(i, 1)
    store.save()
    pushTodos()
    return true
  })

  ipcMain.handle('todos:postpone', (_e, id, minutes) => {
    const t = store.find(id)
    if (!t) return null
    const min = Math.max(1, Number(minutes) || 10)
    const base = Math.max(Date.now(), t.dueAt || 0)
    t.dueAt = base + min * 60000
    t.postpones = (t.postpones || 0) + 1
    delete t.ackDue
    addPoints(-3, `⏳ 미루기: ${t.title}`)
    store.save()
    if (sirenTodoId === id && !sirenEligible(t, Date.now())) closeSiren()
    pushTodos()
    return t
  })

  ipcMain.handle('settings:get', () => store.data.settings)

  ipcMain.handle('settings:set', (_e, patch) => {
    Object.assign(store.data.settings, patch)
    store.save()
    schemaCache = null
    applyLoginItem()
    return store.data.settings
  })

  ipcMain.handle('notion:sync', () => syncNotion())

  ipcMain.handle('report:copy', () => {
    const md = buildDailyReport()
    clipboard.writeText(md)
    return { ok: true, text: md }
  })

  ipcMain.handle('notion:users', async () => {
    const { notionToken } = store.data.settings
    if (!notionToken) return { ok: false, error: '노션 토큰을 먼저 입력하세요' }
    try {
      const users = await notion.listUsers(notionToken)
      return { ok: true, users }
    } catch (err) {
      return { ok: false, error: String(err.message || err) }
    }
  })

  ipcMain.handle('dashboard:open', () => { showDashboard(); return true })

  ipcMain.handle('app:quit', () => { app.quit(); return true })

  ipcMain.handle('reward:close', () => {
    if (rewardWin && !rewardWin.isDestroyed()) rewardWin.destroy()
    return true
  })

  ipcMain.on('postit:mouse', (_e, ignore) => {
    if (postitWin && !postitWin.isDestroyed()) {
      postitWin.setIgnoreMouseEvents(!!ignore, { forward: true })
    }
  })

  // 위젯 드래그: 렌더러의 app-region은 이 창 구성(투명+비포커스)에서 동작하지 않으므로
  // 메인에서 커서를 추적하며 창을 직접 이동
  let dragTimer = null
  ipcMain.on('postit:dragStart', () => {
    if (!postitWin || postitWin.isDestroyed()) return
    const startCursor = screen.getCursorScreenPoint()
    const [wx, wy] = postitWin.getPosition()
    clearInterval(dragTimer)
    dragTimer = setInterval(() => {
      if (!postitWin || postitWin.isDestroyed()) { clearInterval(dragTimer); return }
      const c = screen.getCursorScreenPoint()
      postitWin.setPosition(wx + c.x - startCursor.x, wy + c.y - startCursor.y)
    }, 16)
  })
  ipcMain.on('postit:dragEnd', () => {
    clearInterval(dragTimer)
    dragTimer = null
  })
}

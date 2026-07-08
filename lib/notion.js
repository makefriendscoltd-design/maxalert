const API = 'https://api.notion.com/v1'
const VERSION = '2022-06-28'

async function req(token, method, apiPath, body) {
  const res = await fetch(API + apiPath, {
    method,
    headers: {
      'Authorization': 'Bearer ' + token,
      'Notion-Version': VERSION,
      'Content-Type': 'application/json'
    },
    body: body ? JSON.stringify(body) : undefined
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`Notion API ${res.status}: ${text.slice(0, 300)}`)
  }
  return res.json()
}

// DB 스키마에서 제목/날짜/체크박스/담당자(사람) 속성을 자동 감지
async function getSchema(token, dbId) {
  const db = await req(token, 'GET', `/databases/${dbId}`)
  let titleProp = null, dateProp = null, doneProp = null, personProp = null
  for (const [name, p] of Object.entries(db.properties)) {
    if (p.type === 'title' && !titleProp) titleProp = name
    if (p.type === 'date' && !dateProp) dateProp = name
    if (p.type === 'checkbox' && !doneProp) doneProp = name
    if (p.type === 'people' && !personProp) personProp = name
  }
  if (!titleProp) throw new Error('데이터베이스에 제목(title) 속성이 없습니다')
  if (!dateProp) throw new Error('데이터베이스에 날짜(date) 속성이 없습니다')
  return { titleProp, dateProp, doneProp, personProp }
}

// 워크스페이스 사용자 목록 (담당자 선택용). 봇 제외, 사람만 반환
async function listUsers(token) {
  const users = []
  let cursor
  do {
    const qs = cursor ? `?start_cursor=${cursor}&page_size=100` : '?page_size=100'
    const r = await req(token, 'GET', `/users${qs}`)
    for (const u of r.results || []) {
      if (u.type === 'person') users.push({ id: u.id, name: u.name || '(이름 없음)' })
    }
    cursor = r.has_more ? r.next_cursor : null
  } while (cursor)
  return users
}

// 기간 일정(다중일)을 놓치지 않도록 시작일 기준 조회 범위(뒤로 며칠까지 훑을지)
const LOOKBACK_DAYS = 180

// "YYYY-MM-DD"에서 n일 뺀 날짜 문자열
function daysBefore(dayStr, n) {
  const [y, m, d] = dayStr.split('-').map(Number)
  const dt = new Date(Date.UTC(y, m - 1, d) - n * 86400000)
  const p = x => String(x).padStart(2, '0')
  return `${dt.getUTCFullYear()}-${p(dt.getUTCMonth() + 1)}-${p(dt.getUTCDate())}`
}

// dayStr(오늘)과 겹치는 페이지 조회 — 다중일(기간) 일정 포함.
// 노션 날짜 필터는 '시작일' 기준으로만 비교되므로, 넉넉히 받아 JS에서 겹침을 판정한다.
// assigneeId가 있으면 담당자로 추가 필터링.
async function queryDay(token, dbId, schema, dayStr, nextDayStr, assigneeId) {
  const conds = [
    { property: schema.dateProp, date: { on_or_after: daysBefore(dayStr, LOOKBACK_DAYS) } },
    { property: schema.dateProp, date: { before: nextDayStr } }
  ]
  if (assigneeId && schema.personProp) {
    conds.push({ property: schema.personProp, people: { contains: assigneeId } })
  }
  const filter = { and: conds }

  // 페이지네이션으로 전부 수집 (하루 100건 초과해도 누락 방지)
  const pages = []
  let cursor
  do {
    const body = { filter, page_size: 100 }
    if (cursor) body.start_cursor = cursor
    const r = await req(token, 'POST', `/databases/${dbId}/query`, body)
    pages.push(...r.results)
    cursor = r.has_more ? r.next_cursor : null
  } while (cursor)

  const out = []
  for (const pg of pages) {
    const props = pg.properties
    const dateVal = props[schema.dateProp]?.date
    const start = dateVal && dateVal.start
    if (!start) continue
    const startDay = start.slice(0, 10)
    const endDay = dateVal.end ? dateVal.end.slice(0, 10) : startDay
    // 오늘이 [시작일, 종료일] 안에 들어오는 일정만
    if (!(startDay <= dayStr && dayStr <= endDay)) continue
    const titleParts = props[schema.titleProp]?.title || []
    const title = titleParts.map(t => t.plain_text).join('') || '(제목 없음)'
    // 오늘 시작하는 일정만 시간(사이렌) 적용, 이미 진행 중인 기간 일정은 종일로 처리
    const startsToday = startDay === dayStr
    const hasTime = startsToday && start.length > 10
    const done = schema.doneProp ? !!props[schema.doneProp]?.checkbox : false
    out.push({ id: pg.id, title, dueAt: hasTime ? Date.parse(start) : null, done })
  }
  return out
}

async function setDone(token, pageId, doneProp, done) {
  if (!doneProp) return
  await req(token, 'PATCH', `/pages/${pageId}`, {
    properties: { [doneProp]: { checkbox: !!done } }
  })
}

module.exports = { getSchema, listUsers, queryDay, setDone }

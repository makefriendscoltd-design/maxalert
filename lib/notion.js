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

// 특정 날짜의 페이지 조회. assigneeId가 있으면 담당자로 추가 필터링
async function queryDay(token, dbId, schema, dayStr, nextDayStr, assigneeId) {
  const conds = [
    { property: schema.dateProp, date: { on_or_after: dayStr } },
    { property: schema.dateProp, date: { before: nextDayStr } }
  ]
  if (assigneeId && schema.personProp) {
    conds.push({ property: schema.personProp, people: { contains: assigneeId } })
  }
  const body = {
    filter: { and: conds },
    page_size: 100
  }
  const r = await req(token, 'POST', `/databases/${dbId}/query`, body)
  return r.results.map(pg => {
    const props = pg.properties
    const titleParts = (props[schema.titleProp]?.title) || []
    const title = titleParts.map(t => t.plain_text).join('') || '(제목 없음)'
    const dateVal = props[schema.dateProp]?.date
    const start = dateVal && dateVal.start
    const hasTime = !!(start && start.length > 10)
    const done = schema.doneProp ? !!props[schema.doneProp]?.checkbox : false
    return {
      id: pg.id,
      title,
      dueAt: hasTime ? Date.parse(start) : null,
      done
    }
  })
}

async function setDone(token, pageId, doneProp, done) {
  if (!doneProp) return
  await req(token, 'PATCH', `/pages/${pageId}`, {
    properties: { [doneProp]: { checkbox: !!done } }
  })
}

module.exports = { getSchema, listUsers, queryDay, setDone }

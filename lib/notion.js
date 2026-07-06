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

// DB 스키마에서 제목/날짜/체크박스 속성을 자동 감지
async function getSchema(token, dbId) {
  const db = await req(token, 'GET', `/databases/${dbId}`)
  let titleProp = null, dateProp = null, doneProp = null
  for (const [name, p] of Object.entries(db.properties)) {
    if (p.type === 'title' && !titleProp) titleProp = name
    if (p.type === 'date' && !dateProp) dateProp = name
    if (p.type === 'checkbox' && !doneProp) doneProp = name
  }
  if (!titleProp) throw new Error('데이터베이스에 제목(title) 속성이 없습니다')
  if (!dateProp) throw new Error('데이터베이스에 날짜(date) 속성이 없습니다')
  return { titleProp, dateProp, doneProp }
}

// 특정 날짜의 페이지 조회
async function queryDay(token, dbId, schema, dayStr, nextDayStr) {
  const body = {
    filter: {
      and: [
        { property: schema.dateProp, date: { on_or_after: dayStr } },
        { property: schema.dateProp, date: { before: nextDayStr } }
      ]
    },
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

module.exports = { getSchema, queryDay, setDone }

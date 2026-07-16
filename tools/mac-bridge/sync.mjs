#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const CONFIG_PATH = path.join(SCRIPT_DIR, "config.json");
const LAST_RUN_PATH = path.join(SCRIPT_DIR, "last-run.json");
const LOG_DIR = path.join(SCRIPT_DIR, "logs");
const BACKUP_DIR = path.join(SCRIPT_DIR, "backups");
const LOCK_PATH = path.join(SCRIPT_DIR, ".sync.lock");
const WRITEBACK_LOG_PATH = path.join(SCRIPT_DIR, "writeback-log.json");
const KST_OFFSET_MS = 9 * 60 * 60 * 1000;

const args = new Set(process.argv.slice(2));
const isDryRun = args.has("--dry-run");
const isAuto = args.has("--auto");
const force = args.has("--force") || args.has("--dry-run");

function expandHome(value) {
  if (!value) return value;
  return value.startsWith("~/") ? path.join(os.homedir(), value.slice(2)) : value;
}

function readJson(filePath, fallback) {
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch {
    return fallback;
  }
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

function logLine(message) {
  fs.mkdirSync(LOG_DIR, { recursive: true });
  fs.appendFileSync(path.join(LOG_DIR, "sync.log"), `${new Date().toISOString()} ${message}\n`);
}

function todayKst() {
  return new Date(Date.now() + KST_OFFSET_MS).toISOString().slice(0, 10);
}

function nowKstParts() {
  const d = new Date(Date.now() + KST_OFFSET_MS);
  return {
    date: d.toISOString().slice(0, 10),
    hour: d.getUTCHours(),
    minute: d.getUTCMinutes()
  };
}

function kstDateTime(date, time) {
  return new Date(`${date}T${time}:00+09:00`).getTime();
}

function nextDate(date) {
  const d = new Date(`${date}T00:00:00+09:00`);
  d.setUTCDate(d.getUTCDate() + 1);
  return new Date(d.getTime() + KST_OFFSET_MS).toISOString().slice(0, 10);
}

function getSecret(service) {
  return execFileSync("security", ["find-generic-password", "-w", "-s", service], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"]
  }).trim();
}

async function notionRequest(token, endpoint, options = {}) {
  const res = await fetch(`https://api.notion.com/v1${endpoint}`, {
    method: options.method || "GET",
    headers: {
      Authorization: `Bearer ${token}`,
      "Notion-Version": "2022-06-28",
      "Content-Type": "application/json"
    },
    body: options.body ? JSON.stringify(options.body) : undefined
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    const msg = body.message || body.code || `notion_http_${res.status}`;
    throw new Error(`${endpoint}: ${msg}`);
  }
  return body;
}

async function queryAll(token, databaseId, body) {
  const results = [];
  let cursor;
  do {
    const payload = { page_size: 100, ...body };
    if (cursor) payload.start_cursor = cursor;
    const data = await notionRequest(token, `/databases/${databaseId}/query`, {
      method: "POST",
      body: payload
    });
    results.push(...(data.results || []));
    cursor = data.has_more ? data.next_cursor : null;
  } while (cursor);
  return results;
}

function plainText(chunks = []) {
  return chunks.map((item) => item.plain_text || "").join("").trim();
}

function titleFrom(page, preferredProps) {
  for (const prop of preferredProps) {
    const value = page.properties?.[prop];
    if (value?.type === "title") return plainText(value.title);
  }
  for (const value of Object.values(page.properties || {})) {
    if (value?.type === "title") return plainText(value.title);
  }
  return "제목 없는 항목";
}

function textFrom(page, names) {
  const pieces = [];
  for (const name of names) {
    const value = page.properties?.[name];
    if (!value) continue;
    if (value.type === "title") pieces.push(plainText(value.title));
    if (value.type === "rich_text") pieces.push(plainText(value.rich_text));
    if (value.type === "select") pieces.push(value.select?.name || "");
    if (value.type === "status") pieces.push(value.status?.name || "");
  }
  return pieces.filter(Boolean).join(" ");
}

function dateProp(page, names) {
  for (const name of names) {
    const value = page.properties?.[name];
    if (value?.type === "date" && value.date?.start) return value.date;
  }
  return null;
}

function checkboxProp(page, name) {
  const value = page.properties?.[name];
  return value?.type === "checkbox" ? Boolean(value.checkbox) : false;
}

function peopleIds(page, name) {
  const value = page.properties?.[name];
  if (value?.type !== "people") return [];
  return (value.people || []).map((person) => person.id);
}

function selectName(page, name) {
  const value = page.properties?.[name];
  if (value?.type === "select") return value.select?.name || "";
  if (value?.type === "status") return value.status?.name || "";
  return "";
}

function hasAssignee(page, config) {
  const people = new Set([
    ...peopleIds(page, "담당자"),
    ...peopleIds(page, "담당자_사람")
  ]);
  if (people.has(config.notionUserId)) return true;
  const selected = [selectName(page, "담당자"), selectName(page, "오너")].filter(Boolean);
  return selected.some((name) => config.taskAssigneeNames.includes(name));
}

function scheduleMatchesOwner(page, config) {
  const ids = peopleIds(page, "담당자");
  if (ids.includes(config.notionUserId)) return true;
  return Boolean(config.includeUnassignedSchedules && ids.length === 0);
}

function parseTimeFromText(text) {
  const normalized = String(text || "").replace(/\s+/g, " ");
  const colon = normalized.match(/\b([01]?\d|2[0-3]):([0-5]\d)\b/);
  if (colon) return `${colon[1].padStart(2, "0")}:${colon[2]}`;
  const korean = normalized.match(/(오전|오후|아침|저녁|밤)?\s*(\d{1,2})\s*시\s*(반|([0-5]?\d)\s*분?)?/);
  if (!korean) return null;
  let hour = Number(korean[2]);
  const marker = korean[1] || "";
  let minute = korean[3] === "반" ? 30 : Number(korean[4] || 0);
  if ((marker === "오후" || marker === "저녁" || marker === "밤") && hour < 12) hour += 12;
  if ((marker === "오전" || marker === "아침") && hour === 12) hour = 0;
  if (hour > 23 || minute > 59) return null;
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

function isDateOnly(notionDate) {
  return notionDate && !String(notionDate.start || "").includes("T");
}

function dueMsFromNotionDate(notionDate, fallbackDate, fallbackTime, sourceText) {
  if (!notionDate?.start) return kstDateTime(fallbackDate, fallbackTime);
  if (!isDateOnly(notionDate)) return new Date(notionDate.start).getTime();
  const inferred = parseTimeFromText(sourceText) || fallbackTime;
  return kstDateTime(notionDate.start, inferred);
}

function bumpIfInferredPast(ms, wasInferred, index = 0, slotMinutes = 30, startDelayMinutes = 60) {
  const minFuture = Date.now() + (startDelayMinutes + index * slotMinutes) * 60 * 1000;
  if (wasInferred && ms < minFuture) return minFuture;
  return ms;
}

function bumpPastTask(ms, index = 0, slotMinutes = 30, startDelayMinutes = 60) {
  if (ms >= Date.now()) return ms;
  return Date.now() + (startDelayMinutes + index * slotMinutes) * 60 * 1000;
}

function todoId(prefix, pageId) {
  return `${prefix}_${pageId.replaceAll("-", "").slice(0, 24)}`;
}

function todoForPage({ page, title, date, dueAt, done, source, color }) {
  const id = todoId(source === "task" ? "ntask" : "nsched", page.id);
  const todo = {
    id,
    title,
    color,
    date,
    done,
    createdAt: Date.now(),
    dueAt,
    doneAt: done ? Date.now() : null,
    notionPageId: page.id,
    notionDueAt: dueAt,
    notionDone: done,
    bridgeSource: `notion-${source}`,
    bridgeSyncedAt: new Date().toISOString()
  };
  if (dueAt && dueAt <= Date.now()) todo.ackDue = dueAt;
  return todo;
}

function isOnOrAfter(date, baseDate) {
  return typeof date === "string" && date.length >= 10 && date.slice(0, 10) >= baseDate;
}

function isLocallyDeferred(todo, today) {
  return Boolean(todo.deferredAt) && isOnOrAfter(todo.date, today);
}

function mergeDeferredTodo(todo, next) {
  return {
    ...todo,
    title: next.title,
    color: next.color,
    done: Boolean(todo.done || next.done),
    doneAt: (todo.done || next.done) ? (todo.doneAt || next.doneAt || Date.now()) : null,
    notionDueAt: next.dueAt,
    notionDone: next.done,
    bridgeSyncedAt: next.bridgeSyncedAt
  };
}

function slotTime(config, index) {
  const [h, m] = config.taskStartTime.split(":").map(Number);
  const total = h * 60 + m + index * Number(config.taskSlotMinutes || 30);
  const hour = Math.min(23, Math.floor(total / 60));
  const minute = total % 60;
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

function mergeTodos(data, incoming, today) {
  const byId = new Map(incoming.map((todo) => [todo.id, todo]));
  const existing = Array.isArray(data.todos) ? data.todos : [];
  const merged = [];
  let updated = 0;
  let removed = 0;
  for (const todo of existing) {
    if (todo.bridgeSource?.startsWith("notion-")) {
      const next = byId.get(todo.id);
      if (next && isLocallyDeferred(todo, today)) {
        merged.push(mergeDeferredTodo(todo, next));
        byId.delete(todo.id);
        updated += 1;
      } else if (!next && isLocallyDeferred(todo, today)) {
        merged.push({
          ...todo,
          bridgeSyncedAt: new Date().toISOString()
        });
      } else if (todo.date === today && next) {
        // 2026-07-16 완료기록 보존: 앱에서 완료(done)한 것을 노션 미완료가 덮지 않는다
        // (윈도우 v0.1.7 "동기화가 완료 항목 삭제하던 버그" 수정과 동일 취지)
        const mergedDone = Boolean(todo.done || next.done);
        merged.push({
          ...todo,
          ...next,
          done: mergedDone,
          createdAt: todo.createdAt || next.createdAt,
          doneAt: mergedDone ? (todo.doneAt || next.doneAt || Date.now()) : null
        });
        byId.delete(todo.id);
        updated += 1;
      } else if (todo.date === today && todo.done) {
        // 오늘 완료한 항목이 노션 응답에서 빠져도(완료 체크 역반영·기간 이동 후) 기록은 유지
        merged.push(todo);
      } else if (todo.date === today) {
        removed += 1;
      } else if (!todo.done && todo.date < today) {
        // 2026-07-15 사이렌 폭주 수정: 지난 날짜의 미완료 브리지 항목은 정리한다.
        // 노션이 원본이므로 아직 미완료면 오늘자 incoming 으로 다시 들어오고,
        // 남겨두면 같은 노션 페이지가 날짜만 다른 복제본으로 매일 누적된다.
        // (완료된 항목은 기록/통계 보존을 위해 유지, 로컬 이월(deferred)은 위 분기에서 보존)
        removed += 1;
      } else {
        merged.push(todo);
      }
    } else {
      merged.push(todo);
    }
  }
  const added = [...byId.values()];
  // 2026-07-16 완료버튼 무반응 수정: 과거 날짜의 완료 기록(보존 대상)이 오늘 항목과
  // 같은 id(ntask_<pageId>, 날짜 없음)를 갖고 있으면 앱의 todos_toggle 이 배열 첫
  // 매치(과거 항목)를 토글해 오늘 항목이 안 눌리는 것처럼 보인다.
  // → 오늘 항목이 id 의 유일한 주인이 되도록 과거 항목 id 를 날짜 접미사로 아카이브.
  const todayIds = new Set(
    [...merged, ...added].filter((t) => t.date === today).map((t) => t.id)
  );
  for (const t of merged) {
    if (t.date !== today && todayIds.has(t.id) && !t.id.includes("_a20")) {
      t.id = `${t.id}_a${t.date}`;
    }
  }
  data.todos = [...merged, ...added].sort((a, b) => {
    const ad = a.date || "";
    const bd = b.date || "";
    if (ad !== bd) return ad.localeCompare(bd);
    return (a.dueAt || Number.MAX_SAFE_INTEGER) - (b.dueAt || Number.MAX_SAFE_INTEGER);
  });
  return { added: added.length, updated, removed };
}


// ---- 노션 역반영 (앱 완료 → 노션) ----
// 태스크: "완료" 체크박스 체크. 일정: 단일(하루)이면 "done" 체크,
// 기간 일정(종료일이 오늘 이후)이면 완료 체크 대신 시작일만 내일로 이동(시간 보존)하고
// 종료일은 유지한다 — 노션 쪽 "미완료만 보기" 필터에서 장기 일정 페이지가
// 통째로 사라지는 문제 방지 (2026-07-16 민수 지시).
async function runWriteback(config, today) {
  const dataPath = expandHome(config.maxalertDataPath);
  const data = readJson(dataPath, null);
  if (!data || !Array.isArray(data.todos)) return { checked: 0, applied: 0 };
  const ledger = readJson(WRITEBACK_LOG_PATH, {});
  const targets = data.todos.filter((t) =>
    t && t.bridgeSource && t.bridgeSource.startsWith("notion-") &&
    t.done && t.date === today && t.notionPageId && !t.notionDone &&
    !ledger[`${t.notionPageId}:${t.date}`]
  );
  if (!targets.length) return { checked: 0, applied: 0 };
  const token = getSecret(config.keychainService);
  let applied = 0;
  for (const t of targets) {
    const key = `${t.notionPageId}:${t.date}`;
    try {
      if (t.bridgeSource === "notion-task") {
        await notionRequest(token, `/pages/${t.notionPageId}`, {
          method: "PATCH",
          body: { properties: { "완료": { checkbox: true } } }
        });
        ledger[key] = { at: new Date().toISOString(), action: "task_done" };
      } else {
        const page = await notionRequest(token, `/pages/${t.notionPageId}`);
        const value = page.properties?.["날짜"];
        const d = value?.type === "date" ? value.date : null;
        const endDate = d?.end ? String(d.end).slice(0, 10) : null;
        if (endDate && endDate > today) {
          const startStr = String(d.start || today);
          const newStart = startStr.length > 10 ? nextDate(today) + startStr.slice(10) : nextDate(today);
          const dateBody = { start: newStart, end: d.end };
          if (d.time_zone) dateBody.time_zone = d.time_zone;
          await notionRequest(token, `/pages/${t.notionPageId}`, {
            method: "PATCH",
            body: { properties: { "날짜": { date: dateBody } } }
          });
          ledger[key] = { at: new Date().toISOString(), action: "schedule_shifted", newStart };
        } else {
          await notionRequest(token, `/pages/${t.notionPageId}`, {
            method: "PATCH",
            body: { properties: { done: { checkbox: true } } }
          });
          ledger[key] = { at: new Date().toISOString(), action: "schedule_done" };
        }
      }
      applied += 1;
      logLine(`writeback ${ledger[key].action} ${t.title}`);
    } catch (error) {
      logLine(`writeback error ${t.title}: ${error.message}`);
    }
  }
  if (applied) {
    const cutoff = new Date(Date.now() - 30 * 86400000).toISOString().slice(0, 10);
    for (const k of Object.keys(ledger)) {
      const date = k.split(":").pop();
      if (date && date < cutoff) delete ledger[k];
    }
    writeJson(WRITEBACK_LOG_PATH, ledger);
  }
  return { checked: targets.length, applied };
}

function acquireLock() {
  try {
    fs.writeFileSync(LOCK_PATH, String(process.pid), { flag: "wx" });
  } catch {
    throw new Error("another sync is already running");
  }
}

function releaseLock() {
  try {
    fs.unlinkSync(LOCK_PATH);
  } catch {}
}

async function main() {
  fs.mkdirSync(LOG_DIR, { recursive: true });
  fs.mkdirSync(BACKUP_DIR, { recursive: true });
  const config = readJson(CONFIG_PATH, null);
  if (!config) throw new Error(`missing config: ${CONFIG_PATH}`);
  const now = nowKstParts();
  const today = todayKst();
  const last = readJson(LAST_RUN_PATH, {});
  // 앱 완료 → 노션 역반영은 하루 1회 동기화와 무관하게 매 실행(15분 주기) 시도
  let writeback = { checked: 0, applied: 0 };
  if (!isDryRun) {
    try {
      writeback = await runWriteback(config, today);
    } catch (error) {
      logLine(`writeback error ${error.message}`);
    }
  }
  if (isAuto && !force) {
    if (now.hour < Number(config.autoRunHour || 8)) {
      console.log(JSON.stringify({ ok: true, skipped: "before_auto_run_hour", today, writeback }));
      return;
    }
    if (last.lastSuccessDate === today) {
      console.log(JSON.stringify({ ok: true, skipped: "already_synced_today", today, writeback }));
      return;
    }
  }

  acquireLock();
  try {
    const token = getSecret(config.keychainService);
    const tomorrow = nextDate(today);
    const schedules = await queryAll(token, config.scheduleDatabaseId, {
      filter: {
        and: [
          { property: "날짜", date: { on_or_after: today } },
          { property: "날짜", date: { before: tomorrow } }
        ]
      },
      sorts: [{ property: "날짜", direction: "ascending" }]
    });
    const taskFilters = [
      { property: "완료", checkbox: { equals: false } },
      {
        or: [
          { property: "마감일", date: { on_or_before: today } },
          { property: "검토일", date: { on_or_before: today } }
        ]
      }
    ];
    const tasks = await queryAll(token, config.taskDatabaseId, {
      filter: { and: taskFilters },
      sorts: [{ property: "마감일", direction: "ascending" }]
    });

    const incoming = [];
    for (const page of schedules.filter((item) => scheduleMatchesOwner(item, config))) {
      const title = titleFrom(page, ["일정 메모"]);
      const text = `${title} ${textFrom(page, ["분류", "장소", "유형"])}`;
      const notionDate = dateProp(page, ["날짜"]);
      const inferred = isDateOnly(notionDate);
      const dueAt = bumpIfInferredPast(
        dueMsFromNotionDate(notionDate, today, config.defaultScheduleTime, text),
        inferred && !parseTimeFromText(text),
        incoming.length,
        Number(config.taskSlotMinutes || 30),
        Number(config.inferredStartDelayMinutes || 60)
      );
      incoming.push(todoForPage({
        page,
        title: `[일정] ${title}`,
        date: today,
        dueAt,
        done: checkboxProp(page, "done"),
        source: "schedule",
        color: "blue"
      }));
    }

    const taskCandidates = tasks.filter((item) => hasAssignee(item, config)).slice(0, Number(config.maxTaskCount || 12));
    taskCandidates.forEach((page, index) => {
      const title = titleFrom(page, ["태스크명"]);
      const text = `${title} ${textFrom(page, ["출처", "막힘 이유", "상태", "우선순위"])}`;
      const notionDate = dateProp(page, ["마감일", "검토일"]);
      const inferred = !notionDate || isDateOnly(notionDate);
      // 2026-07-15 사이렌 폭주 수정: 태스크는 노션에 명시적 일시가 있을 때만 사이렌 대상.
      // 인공 슬롯(bumpPastTask/bumpIfInferredPast)은 매일 미래 dueAt+ackDue 없는 항목을
      // 양산해 낮에 앱을 켜는 순간 연쇄 사이렌을 만들었다. 시간 미지정 태스크는
      // dueAt 없이 포스트잇 표시만 한다.
      const dueAt = inferred ? null : new Date(notionDate.start).getTime();
      incoming.push(todoForPage({
        page,
        title: `[태스크] ${title}`,
        date: today,
        dueAt,
        done: checkboxProp(page, "완료"),
        source: "task",
        color: selectName(page, "우선순위") === "긴급" ? "pink" : "yellow"
      }));
    });

    const dataPath = expandHome(config.maxalertDataPath);
    const data = readJson(dataPath, null);
    if (!data) throw new Error(`missing MaxAlert data: ${dataPath}`);
    // 사용자가 앱에서 삭제(억제)한 노션 페이지는 다시 넣지 않는다
    const suppressed = new Set(Array.isArray(data.suppressedNotionIds) ? data.suppressedNotionIds : []);
    const kept = incoming.filter((t) => !suppressed.has(t.notionPageId));
    const suppressedCount = incoming.length - kept.length;
    incoming.length = 0;
    incoming.push(...kept);
    const beforeCount = Array.isArray(data.todos) ? data.todos.length : 0;
    const changes = mergeTodos(data, incoming, today);
    const afterCount = Array.isArray(data.todos) ? data.todos.length : 0;
    const summary = {
      ok: true,
      dryRun: isDryRun,
      today,
      scheduleFetched: schedules.length,
      taskFetched: tasks.length,
      incoming: incoming.length,
      suppressed: suppressedCount,
      writeback,
      beforeCount,
      afterCount,
      changes,
      preview: incoming.map((todo) => ({
        title: todo.title,
        time: todo.dueAt ? new Date(todo.dueAt + KST_OFFSET_MS).toISOString().slice(11, 16) : null,
        done: todo.done,
        source: todo.bridgeSource
      }))
    };

    if (!isDryRun) {
      const backupPath = path.join(BACKUP_DIR, `maxalert-data-${today}-${Date.now()}.json`);
      fs.copyFileSync(dataPath, backupPath);
      writeJson(dataPath, data);
      writeJson(LAST_RUN_PATH, {
        lastSuccessAt: new Date().toISOString(),
        lastSuccessDate: today,
        incoming: incoming.length,
        backupPath
      });
      logLine(`synced today=${today} incoming=${incoming.length} added=${changes.added} updated=${changes.updated} removed=${changes.removed}`);
    } else {
      logLine(`dry-run today=${today} incoming=${incoming.length} added=${changes.added} updated=${changes.updated} removed=${changes.removed}`);
    }
    console.log(JSON.stringify(summary, null, 2));
  } finally {
    releaseLock();
  }
}

main().catch((error) => {
  logLine(`error ${error.message}`);
  console.error(JSON.stringify({ ok: false, error: error.message }, null, 2));
  process.exit(1);
});

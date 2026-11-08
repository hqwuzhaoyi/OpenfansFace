import {isReviewAvailable} from './review-continuity-model'
import fs from 'fs'
import path from 'path'
import Database from 'better-sqlite3'
import { BASE } from './env'
import { readJson } from './jsonio'
import { loadQueue, loadArchive } from './review-store'
import { readVideoSources, videoSource } from './video-sources'
import { ReviewError } from './review-logic'
import { nativeJsonRequest } from './native-json-transport'
import { canonicalPersonFields } from './person-names'
import { type ProcessingTask } from './processing-model'

const FILE = path.join(BASE, 'processing/processing.db')
export function processingDb() {
  if (!fs.existsSync(FILE)) return null
  const db = new Database(FILE, { readonly: true, fileMustExist: true })
  db.pragma('busy_timeout = 15000')
  return db
}
export function processingState() {
  const heartbeat = readJson<{ at?: number }>(path.join(BASE, 'processing/heartbeat.json'), {})
  const age = Date.now() / 1000 - (heartbeat.at || 0)
  const db = processingDb()
  if (!db) return { online: false, paused: false, last_discovery: null, references: null, tasks: [] as ProcessingTask[], events: [] as any[] }
  try {
    const settings = Object.fromEntries((db.prepare('SELECT key,value FROM settings').all() as { key: string; value: string }[]).map(r => [r.key, JSON.parse(r.value)]))
    const queue = loadQueue(), handled = [...queue, ...loadArchive()]
    const ids = new Set(handled.map(v => v.vid)), sources = readVideoSources()
    const paths = new Set(queue.map(v => videoSource(v.vid, sources)).filter(Boolean))
    const tasks = (db.prepare("SELECT * FROM tasks WHERE stage!='excluded' ORDER BY priority DESC,is_new DESC,created_at DESC,vid").all() as any[])
      .filter(v => !ids.has(v.vid) && !paths.has(v.path)).map(v => ({ ...v, data: canonicalPersonFields(JSON.parse(v.data)) }) as ProcessingTask)
    return { online: age >= -30 && age < 90, paused: !!settings.paused, last_discovery: settings.last_discovery || null,
      references: settings.references || null, tasks, events: db.prepare('SELECT * FROM events ORDER BY id DESC LIMIT 12').all() as any[] }
  } finally { db.close() }
}

export function processingOverview(params: URLSearchParams) {
  const state = processingState(), counts: Record<string, number> = {}
  for (const task of state.tasks) {const stage=task.manual?'review':task.stage;counts[stage] = (counts[stage] || 0) + 1}
  counts.review=state.tasks.filter(isReviewAvailable).length
  const stage = params.get('stage') || '', batch = params.get('batch') || '', query = (params.get('q') || '').toLowerCase()
  let rows = state.tasks.filter(t => (!stage || (stage === 'review' ? isReviewAvailable(t) : stage==='error' ? t.stage==='error'&&!t.manual : stage === 'face' ? ['face','face_queued'].includes(t.stage) : stage === 'queued' ? ['queued','retry'].includes(t.stage) : t.stage === stage)) && (!batch || t.batch_id === batch) && (!query || `${t.name} ${t.path}`.toLowerCase().includes(query)))
  const limit = 25, page = Math.min(Math.max(0, Number(params.get('page')) || 0), Math.max(0, Math.ceil(rows.length / limit) - 1)), total = rows.length
  rows = rows.slice(page * limit, (page + 1) * limit)
  const batches = new Map<string, { id: string; total: number; ready: number; error: number; running: number }>()
  for (const task of state.tasks) {
    const b = batches.get(task.batch_id) || { id: task.batch_id, total: 0, ready: 0, error: 0, running: 0 }
    b.total++; b.ready += Number(isReviewAvailable(task)); b.error += Number(task.stage === 'error'&&!task.manual); b.running += Number(['text','face'].includes(task.stage)); batches.set(b.id, b)
  }
  const { tasks, ...summary } = state
  return { ...summary, counts, total, page, limit, all_count: tasks.length, task: params.has('vid') ? tasks.find(t=>t.vid===params.get('vid')) || null : undefined, rows: rows.map(t => ({ ...t, data: { kind: t.data.kind, suggested: t.data.suggested, face_skipped: t.data.face_skipped } })), batches: [...batches.values()].sort((a,b) => b.id.localeCompare(a.id)).slice(0,20) }
}

export async function processingAction(action: string, vids: string[] = [], note?: string) {
  return nativeJsonRequest<{ ok: boolean }>('v1/metadata', {
    command: 'processing-control',
    body: { action, vids, ...(note === undefined ? {} : { note }) },
  }, {
    label: '自动分析控制',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

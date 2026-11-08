import {isReviewAvailable} from './review-continuity-model'
import fs from 'fs'
import path from 'path'
import Database from 'better-sqlite3'
import { BASE } from './env'
import { readJson } from './jsonio'
import { loadQueue, loadArchive } from './review-store'
import { readVideoSources, videoSource } from './video-sources'
import { ReviewError } from './review-logic'
import { loadClusters } from './integration'
import { canonicalPersonFields } from './person-names'
import { type ProcessingTask, type ProcessingStage, groupReviewTasks, reviewBucket, reviewGroup } from './processing-model'

const FILE = path.join(BASE, 'processing/processing.db')
export function processingDb(write = false) {
  if (!fs.existsSync(FILE)) return null
  const db = new Database(FILE, { readonly: !write, fileMustExist: true })
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

export function reviewData(params: URLSearchParams) {
  const state = processingState(), view = params.get('view') || 'all', query = (params.get('q') || '').toLowerCase(), folder = params.get('folder') || ''
  const ready = state.tasks.filter(t => isReviewAvailable(t))
  const counts = { all: ready.filter(t => !t.deferred).length, suggested: 0, conflict: 0, none: 0, manual: 0, deferred: ready.filter(t => t.deferred).length }
  for (const t of ready.filter(t => !t.deferred)) counts[reviewBucket(t)]++
  let rows = ready.filter(t => view === 'deferred' ? !!t.deferred : !t.deferred && (view === 'all' || reviewBucket(t) === view))
  const person=params.get('person'), cluster=params.get('cluster')
  if(person)rows=rows.filter(t=>t.data.candidates?.some(c=>c.name===person))
  if(cluster){const ids=new Set((loadClusters().find(c=>c.cid===cluster)?.vids||[]).map((v:any)=>v.vid));rows=rows.filter(t=>ids.has(t.vid))}
  // Groups remain stable while filtering files within the current group.
  const groups = groupReviewTasks(rows)
  const focus = params.get('vid'), target = rows.find(t => t.vid === focus)
  const requested = target ? reviewGroup(target).key : params.get('group') || (person ? 'person:' + person : '')
  const group = requested ? groups.find(g => g.key === requested) || null : groups[0] || null
  rows = rows.filter(t => group && reviewGroup(t).key === group.key && (!folder || path.dirname(t.path) === folder) && (!query || `${t.name} ${t.path} ${(t.data.candidates || []).map(c => c.name).join(' ')}`.toLowerCase().includes(query)))
  const limit = 24, focusIndex = target ? rows.findIndex(t => t.vid === target.vid) : -1
  const page = focusIndex >= 0 ? Math.floor(focusIndex / limit) : Math.min(Math.max(0, Number(params.get('page')) || 0), Math.max(0, Math.ceil(rows.length / limit) - 1))
  return { counts, groups, group, total: rows.length, rows: rows.slice(page * limit, (page + 1) * limit), page, limit, focus_missing:!!focus&&!target,
    waiting: state.tasks.filter(t => t.stage!=='review'&&!t.manual).length, online: state.online,
    folders: [...new Set(ready.map(t => path.dirname(t.path)))].sort((a,b) => a.localeCompare(b,'zh-CN')) }
}

export function processingAction(action: string, vids: string[] = [], note?: string) {
  const db = processingDb(true)
  if (!db) throw new ReviewError('分析服务尚未初始化', 503)
  try {
    const at = Date.now() / 1000
    db.transaction(() => {
      if (['pause','resume'].includes(action)) {
        db.prepare("INSERT INTO settings(key,value) VALUES('paused',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(JSON.stringify(action === 'pause'))
        db.prepare('INSERT INTO events(at,kind,message) VALUES(?,?,?)').run(at,action,action==='pause'?'暂停自动分析':'恢复自动分析')
        return
      }
      const eligible = new Set(processingState().tasks.map(t => t.vid))
      if (vids.some(vid => !eligible.has(vid))) throw new ReviewError('视频状态已变化，请刷新列表', 409)
      for (const vid of vids) {
        const task = db.prepare('SELECT stage,manual FROM tasks WHERE vid=?').get(vid) as {stage:string;manual:number}
        if (action === 'priority') db.prepare('UPDATE tasks SET priority=priority+1,updated_at=? WHERE vid=?').run(at,vid)
        else if (action === 'retry') {
          if (!['error','retry'].includes(task.stage)) throw new ReviewError('仅异常或等待重试的任务可以重试',409)
          db.prepare("UPDATE tasks SET stage='retry',text_attempts=0,face_attempts=0,next_retry=0,error=NULL,manual=0,updated_at=? WHERE vid=?").run(at,vid)
        } else if (action === 'takeover') {
          if (task.stage !== 'error') throw new ReviewError('仅异常任务可以接管',409)
          db.prepare('UPDATE tasks SET manual=1,deferred=0,updated_at=? WHERE vid=?').run(at,vid)
        } else if (['defer','restore','note'].includes(action)) {
          if (task.stage !== 'review' && !task.manual) throw new ReviewError('视频尚未就绪',409)
          if (action === 'note') db.prepare('UPDATE tasks SET note=?,updated_at=? WHERE vid=?').run(note || '',at,vid)
          else db.prepare('UPDATE tasks SET deferred=?,updated_at=? WHERE vid=?').run(Number(action === 'defer'),at,vid)
        } else throw new ReviewError('未知操作')
        db.prepare('INSERT INTO events(at,vid,kind,message) VALUES(?,?,?,?)').run(at,vid,action,{priority:'优先处理',retry:'人工重试',takeover:'人工接管',defer:'暂时跳过',restore:'恢复核对',note:'更新备注'}[action] || action)
      }
    })()
  } finally { db.close() }
}

export function watermarkRuleChanged(key:string) {
  const db=processingDb(true)
  if(!db)return
  try{db.prepare(`UPDATE tasks SET stage='queued',retry_stage='text',text_attempts=0,face_attempts=0,next_retry=0,priority=MAX(priority,1),updated_at=?
    WHERE stage='review' AND manual=0 AND EXISTS (SELECT 1 FROM json_each(tasks.data,'$.watermark.candidates') WHERE json_extract(value,'$.key')=?)`).run(Date.now()/1000,key)}finally{db.close()}
}

export function restoreReviewTasks(vids:string[]) {
  const db=processingDb(true)
  if(!db)return
  try{db.transaction(()=>{
    for(const vid of vids){
      const row=db.prepare('SELECT * FROM tasks WHERE vid=?').get(vid) as any
      if(!row)continue
      let valid=false
      try{const stat=fs.statSync(row.path,{bigint:true});valid=Number(stat.size)===row.size&&stat.mtimeNs.toString()===row.mtime_ns}catch{}
      if(valid)db.prepare('UPDATE tasks SET stage=?,deferred=0,updated_at=? WHERE vid=?').run(row.manual?'error':JSON.parse(row.data).ready?'review':'queued',Date.now()/1000,vid)
    }
    db.prepare("INSERT INTO settings VALUES ('discovery_requested',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").run(JSON.stringify(Date.now()/1000))
  })()}finally{db.close()}
}

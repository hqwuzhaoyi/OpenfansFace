import fs from 'fs'
import path from 'path'
import { randomUUID } from 'crypto'
import { NextRequest, NextResponse } from 'next/server'
import { acquireReviewLock, queuedWithOrigin, loadArchive, saveArchive } from '@/lib/review-store'
import { buildPlan } from '@/lib/review-plan'
import { saveQueue, writeJson } from '@/lib/jsonio'
import { validateVids, ReviewError, type ArchivedItem } from '@/lib/review-logic'
import { MANIFESTS_D, STAGE } from '@/lib/env'
import { identicalFiles, moveExclusive } from '@/lib/archive-files'
import { toWin } from '@/lib/paths'
import { peachExec } from '@/lib/peach'
import { invalidateIntegrationCache } from '@/lib/integration'

export async function POST(req: NextRequest) {
  let release: (()=>void) | undefined
  try {
    const body = await req.json(), vids = validateVids(body.vids)
    release = acquireReviewLock()
    const plan = buildPlan()
    if (body.version !== plan.version) throw new ReviewError('待归档内容或文件状态已变化，请刷新预览后再执行', 409)
    const ids = new Set(vids), selected = plan.rows.filter(r=>ids.has(r.vid))
    if (selected.length !== vids.length || selected.some(r=>r.status!=='ok')) throw new ReviewError('只能归档检查通过的视频，请刷新并处理异常项', 409)
    fs.mkdirSync(MANIFESTS_D, { recursive: true })
    const mf = path.join(MANIFESTS_D, `apply-${Date.now()}-${randomUUID().slice(0,8)}.json`)
    const manifest: any = { at: new Date().toISOString(), status: 'moving', moves: [], ledger: [], errors: [] }
    const stageBatch = path.join(STAGE, 'archive-duplicates', randomUUID())
    writeJson(mf, manifest)
    const completed: ArchivedItem[] = []
    for (const item of selected) {
      const src = item.src!, dst = item.dst!
      try {
        const source = fs.statSync(src)
        if (!source.isFile() || source.size !== item.size || source.mtimeMs !== item.mtime) throw new Error('原文件已变化，请重新核对')
        const duplicate = item.operation === 'stage-duplicate'
        if (duplicate) {
          const target = fs.statSync(dst)
          if (!target.isFile() || target.size !== item.dst_size || target.mtimeMs !== item.dst_mtime) throw new Error('目标文件已变化，请重新核对')
          if (!await identicalFiles(src, dst)) throw new Error('完整内容校验不一致，源文件未移动。请改选“保留两份”')
          const checkedSource = fs.statSync(src), checkedTarget = fs.statSync(dst)
          if (checkedSource.size !== item.size || checkedSource.mtimeMs !== item.mtime || checkedTarget.size !== item.dst_size || checkedTarget.mtimeMs !== item.dst_mtime) throw new Error('校验后文件状态已变化，请重新核对')
        } else if (fs.existsSync(dst)) throw new Error('目标文件已出现，请核对冲突')
        const actualDst = duplicate ? path.join(stageBatch, randomUUID(), path.basename(src)) : dst
        moveExclusive(src, actualDst)
        manifest.moves.push({ src, dst: actualDst, canonical_dst: dst, operation: item.operation, win_src: toWin(src), original_vid: item.vid, win_dst: toWin(actualDst) })
        manifest.ledger.push({ from: toWin(src), to: toWin(actualDst), creator: item.creator, canonical: duplicate ? toWin(dst) : undefined })
        completed.push({ ...item, dst, win_dst: toWin(dst), manifest: mf, ...(duplicate ? {staged_path: actualDst} : {}) })
        writeJson(mf, manifest)
      } catch (e: any) { manifest.errors.push(`${item.name}: ${e.message}`) }
    }
    if (completed.length) {
      manifest.status = 'updating_ledger'; writeJson(mf, manifest)
      const payload = Buffer.from(JSON.stringify(manifest.ledger)).toString('base64')
      const script = `import json,sqlite3,base64,datetime
rows=json.loads(base64.b64decode('${payload}'))
con=sqlite3.connect('/data/database/ledger.db',timeout=120)
now=datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
try:
 con.execute('BEGIN IMMEDIATE')
 for m in rows:
  old=con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['from'],)).fetchall()
  if len(old)!=1: raise RuntimeError('source asset not unique or missing')
  target=con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['to'],)).fetchall()
  if target: raise RuntimeError('target asset already exists')
  canonical=None
  if m.get('canonical'):
   canonical=con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['canonical'],)).fetchall()
   if len(canonical)!=1: raise RuntimeError('canonical target asset not unique or missing')
  e=con.execute("SELECT id FROM entity WHERE kind='creator' AND canonical_name=?",(m['creator'],)).fetchone()
  if e: eid=e[0]
  else: eid=con.execute("INSERT INTO entity (kind,canonical_name,normalized_name,metadata_json,created_at,updated_at) VALUES ('creator',?,?,?,?,?)",(m['creator'],m['creator'].lower(),json.dumps({'source':'face-curation-8919'}),now,now)).lastrowid
  aid=old[0][0]
  con.execute('UPDATE asset SET path=? WHERE id=?',(m['to'],aid))
  for linked in ([aid,canonical[0][0]] if canonical else [aid]):
   if not con.execute("SELECT 1 FROM asset_entity WHERE asset_id=? AND entity_id=? AND role='creator'",(linked,eid)).fetchone():
    con.execute("INSERT INTO asset_entity (asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES (?,?,?,?,?,?,?,?)",(linked,eid,'creator','face-curation-8919',0.95,'{}',now,now))
 con.commit()
 print(json.dumps({'updated':len(rows)}))
except Exception:
 con.rollback()
 raise
finally: con.close()
`
      let result: {out:string;code:number;err:string}
      try { result = await peachExec(script) } catch(e:any) { result = {out:'',code:-1,err:e.message} }
      let updated = -1
      try { updated = JSON.parse(result.out).updated } catch {}
      if (result.code !== 0 || updated !== completed.length) {
        // Ledger transaction did not commit: return files to their original paths and keep every task pending.
        manifest.errors.push('Peach 账本更新失败，本轮文件正在恢复原位置')
        for (const mv of [...manifest.moves].reverse()) {
          try { moveExclusive(mv.dst, mv.src); mv.restored = true }
          catch (e:any) { manifest.errors.push(`恢复失败 ${path.basename(mv.src)}: ${e.message}`) }
        }
        manifest.status = manifest.moves.every((m:any)=>m.restored) ? 'rolled_back' : 'recovery_needed'
        manifest.ledger_error = result.err
        writeJson(mf, manifest)
        return NextResponse.json({ ok: false, moved: 0, errors: manifest.errors, manifest: mf, remaining: queuedWithOrigin().length })
      }
      manifest.ledger_remapped = updated
      const history = new Map(loadArchive().map(v=>[v.vid,v]))
      completed.forEach(v=>history.set(v.vid,v))
      saveArchive([...history.values()])
      const succeeded = new Set(completed.map(v=>v.vid))
      saveQueue(queuedWithOrigin().filter(v=>!succeeded.has(v.vid)))
    }
    manifest.status = 'complete'; writeJson(mf, manifest)
    invalidateIntegrationCache()
    return NextResponse.json({ ok: !manifest.errors.length, moved: completed.length, staged: completed.filter(v=>v.staged_path).length, ledger: manifest.ledger_remapped || 0, errors: manifest.errors, manifest: mf, remaining: queuedWithOrigin().length })
  } catch (e:any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}

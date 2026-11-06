import fs from 'fs'
import path from 'path'
import { BASE } from './env'
import { readJson, writeJson, loadQueue, loadDecisions } from './jsonio'
import { ReviewError, type ArchivedItem, type QueueItem } from './review-logic'
import { loadClusters, loadReport } from './integration'

export const ARCHIVE_F = path.join(BASE, 'archive-history.json')
export const loadArchive = () => readJson<ArchivedItem[]>(ARCHIVE_F, [])
export const saveArchive = (items: ArchivedItem[]) => writeJson(ARCHIVE_F, items)

/** Shared across workers and production/shadow instances. Never silently discard a live lock. */
export function acquireReviewLock() {
  const lock = path.join(BASE, '.review-write.lock')
  let fd: number
  try { fd = fs.openSync(lock, 'wx') }
  catch (e: any) {
    if (e.code === 'EEXIST') throw new ReviewError('正在保存或归档，请稍后重试', 409)
    throw e
  }
  fs.writeFileSync(fd, JSON.stringify({ pid: process.pid, at: new Date().toISOString() }))
  return () => { fs.closeSync(fd); fs.rmSync(lock, { force: true }) }
}
export function queuedWithOrigin(): QueueItem[] {
  const byVideo = new Map<string, string>()
  for (const c of loadClusters()) for (const v of c.vids || []) byVideo.set(v.vid, c.cid)
  return loadQueue().map((item: QueueItem) => {
    if (item.return_href) return item
    const cid = byVideo.get(item.vid)
    return { ...item, source: item.source || 'legacy', source_id: cid || '', return_href: cid ? `/clusters?cid=${encodeURIComponent(cid)}` : '/clusters' }
  })
}
export function resolvedVids(): Set<string> {
  return new Set([...loadQueue(), ...loadArchive()].map(v => v.vid))
}
export function videoInfo(): Map<string, any> {
  const out = new Map<string, any>()
  for (const i of loadReport().identities || []) for (const v of i.vids || []) out.set(v.vid, v)
  for (const c of loadClusters()) for (const v of c.vids || []) out.set(v.vid, v)
  return out
}
export { loadQueue, loadDecisions }

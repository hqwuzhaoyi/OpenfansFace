import fs from "fs"
import path from "node:path"
import { BASE } from "./env"
import { randomUUID } from "crypto"
import { QUEUE_F, DECISIONS_F, OVR_F } from "./env"
import { canonicalPersonFields, personRenamesStamp } from './person-names'

const nativeOwned = new Set(['people-deleted.json','report.json','claims-detail.json','promoted.json','clusters.json','integration-queue.json','review-decisions.json','archive-history.json','people-manual.json','template-overrides.json','watermarks/mappings.json','watermarks/results.json','person-rejections.json','video-path-overrides.json','person-renames.json','external-profiles/manual-links.json','external-profiles/reviewed-links.json'])

type CacheEntry = { fp: string; value: any; canonicalStamp?: string; canonicalValue?: any }
const jsonCache = new Map<string, CacheEntry>()
function statFp(p: string): string | null {
  try {
    const s = fs.statSync(/*turbopackIgnore: true*/ p)
    return `${s.mtimeMs}:${s.size}:${s.ino}`
  } catch {
    return null
  }
}

export function readJson<T = any>(p: string, fallback: T): T {
  const fp = statFp(p)
  if (!fp) {
    jsonCache.delete(p)
    return fallback
  }
  const hit = jsonCache.get(p)
  if (hit && hit.fp === fp) return hit.value as T
  try {
    const value = JSON.parse(fs.readFileSync(/*turbopackIgnore: true*/ p, "utf8")) as T
    if (jsonCache.size > 256) jsonCache.clear()
    jsonCache.set(p, { fp, value })
    return value
  } catch { return fallback }
}
export function readCanonicalJson<T = any>(p: string, fallback: T): T {
  const fp = statFp(p)
  if (!fp) {
    jsonCache.delete(p)
    return canonicalPersonFields(fallback)
  }
  const raw = readJson<T>(p, fallback)
  const hit = jsonCache.get(p)
  let stamp = 'raw'
  try { stamp = personRenamesStamp() } catch {}
  if (hit && hit.fp === fp && hit.canonicalStamp === stamp && hit.canonicalValue !== undefined) {
    return hit.canonicalValue as T
  }
  const canonicalValue = canonicalPersonFields(raw)
  if (hit && hit.fp === fp) {
    hit.canonicalStamp = stamp
    hit.canonicalValue = canonicalValue
  }
  return canonicalValue
}
export function writeJson(p: string, data: any) {
  if(nativeOwned.has(path.relative(BASE,p)))throw Object.assign(new Error('共享资料必须通过 Rust 写入协调器保存'),{status:503,code:'native_writer_required'})
  const tmp = `${p}.${randomUUID()}.tmp`
  try {
    fs.writeFileSync(/*turbopackIgnore: true*/ tmp, JSON.stringify(data, null, 1), "utf8")
    fs.renameSync(/*turbopackIgnore: true*/ tmp, /*turbopackIgnore: true*/ p)
    jsonCache.delete(p)
  } finally { fs.rmSync(/*turbopackIgnore: true*/ tmp, { force: true }) }
}
export const loadQueue = () => readCanonicalJson<any[]>(QUEUE_F, [])
export const saveQueue = (q: any[]) => writeJson(QUEUE_F, q)
export const loadDecisions = () => readJson<{ approved: string[]; rejected: string[] }>(DECISIONS_F, { approved: [], rejected: [] })
export const saveDecisions = (d: any) => writeJson(DECISIONS_F, d)
export const loadOverride = () => readJson<Record<string, any>>(OVR_F, {})
export const saveOverride = (o: any) => writeJson(OVR_F, o)

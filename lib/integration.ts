import fs from "fs"
import path from "path"
import os from "os"
import { BASE, CREATORS, MYTG, STAGE } from "./env"
import { readJson, readCanonicalJson, loadQueue, saveQueue, loadDecisions, saveDecisions, loadOverride, saveOverride } from "./jsonio"

export { loadQueue, saveQueue, loadDecisions, saveDecisions, loadOverride, saveOverride }
import { copyPeachLedger } from "./peach"
import Database from "better-sqlite3"
import { canonicalPersonFields } from './person-names'
import { invalidatePersonStats } from './person-library-stats'

const cache: { ledgerTs: number; walkTs: number; integrated?: Record<string, any> } = {
  ledgerTs: 0, walkTs: 0,
}
let walkGeneration = 0
let pendingWalk: Promise<Record<string, { files: number; gb: number }>> | undefined
let pendingLedger: Promise<string> | undefined

/** The network filesystem can take seconds; never block the server event loop while listing people. */
export async function integratedStatsAsync(): Promise<Record<string, { files: number; gb: number }>> {
  if (cache.integrated && Date.now()/1000-cache.walkTs < 600) return cache.integrated
  if (pendingWalk) return pendingWalk
  const generation = walkGeneration
  const work = (async () => {
    const dirs: Record<string, { files: number; gb: number }> = {}
    const entries = (await fs.promises.readdir(CREATORS, {withFileTypes:true})).filter(e=>e.isDirectory())
    let cursor = 0
    // Bound concurrent NAS operations instead of creating one promise per media file.
    await Promise.all(Array.from({length:Math.min(4,entries.length)},async()=>{
      while (cursor < entries.length) {
        const entry=entries[cursor++], stack=[path.join(CREATORS,entry.name)]
        let files=0, bytes=0
        while (stack.length) {
          const current=stack.pop()!
          for (const child of await fs.promises.readdir(current,{withFileTypes:true})) {
            const filename=path.join(current,child.name)
            if(child.isDirectory()) stack.push(filename)
            else {
              try { const stat=await fs.promises.stat(filename); if(stat.isFile()){ files++; bytes+=stat.size } }
              catch(e) { if(!['ENOENT','ENOTDIR'].includes((e as NodeJS.ErrnoException).code||'')) throw e }
            }
          }
        }
        dirs[entry.name]={files,gb:+(bytes/2**30).toFixed(1)}
      }
    }))
    if(generation===walkGeneration){cache.integrated=dirs;cache.walkTs=Date.now()/1000}
    return dirs
  })()
  pendingWalk=work
  try { return await work } finally { if(pendingWalk===work)pendingWalk=undefined }
}

export async function peachLedgerPath(): Promise<string> {
  const p = path.join(BASE, "ledger-readonly.db")
  if (Date.now() / 1000 - cache.ledgerTs < 300) return p
  if (pendingLedger) return pendingLedger
  const generation=walkGeneration
  const work=(async()=>{
    try { await copyPeachLedger(p); if(generation===walkGeneration)cache.ledgerTs = Date.now() / 1000 }
    catch (e) { console.error("ledger copy failed", e) }
    return p
  })()
  pendingLedger=work
  try { return await work } finally { if(pendingLedger===work)pendingLedger=undefined }
}

export function walkStats(top: string): { files: number; bytes: number } {
  let n = 0, total = 0
  const stack = [top]
  while (stack.length) {
    const cur = stack.pop()!
    let entries: fs.Dirent[]
    try { entries = fs.readdirSync(/*turbopackIgnore: true*/ cur, { withFileTypes: true }) } catch { continue }
    for (const e of entries) {
      const p = path.join(cur, e.name)
      if (e.isDirectory()) stack.push(p)
      else {
        try { total += fs.statSync(/*turbopackIgnore: true*/ p).size; n++ } catch { /* pass */ }
      }
    }
  }
  return { files: n, bytes: total }
}

export function integratedStats(): Record<string, { files: number; gb: number }> {
  if (cache.integrated && Date.now() / 1000 - cache.walkTs < 600) return cache.integrated
  const dirs: Record<string, { files: number; gb: number }> = {}
  let entries: fs.Dirent[]
  try { entries = fs.readdirSync(/*turbopackIgnore: true*/ CREATORS, { withFileTypes: true }) } catch { entries = [] }
  for (const e of entries) {
    if (e.isDirectory()) {
      const s = walkStats(path.join(CREATORS, e.name))
      dirs[e.name] = { files: s.files, gb: +(s.bytes / 2 ** 30).toFixed(1) }
    }
  }
  cache.integrated = dirs
  cache.walkTs = Date.now() / 1000
  return dirs
}

export function stagingStats() {
  const s = walkStats(STAGE)
  return { files: s.files, gb: +(s.bytes / 2 ** 30).toFixed(1), path: STAGE }
}

export interface Report { stats: any; identities: any[] }
export function loadReport(): Report {
  return readCanonicalJson<Report>(path.join(BASE, "report.json"), { stats: {}, identities: [] })
}
export function loadClaims(): any[] {
  return readCanonicalJson<any[]>(path.join(BASE, "claims-detail.json"), [])
}
export function loadClusters(): any[] {
  return readJson<any[]>(path.join(BASE, "clusters.json"), [])
}
export function diskUsage(mount: string) {
  try {
    return fs.statfsSync(/*turbopackIgnore: true*/ mount)
  } catch { return null }
}

export async function peachPersons() {
  const p = await peachLedgerPath()
  let links: Record<string, number> = {}
  const aliases: Record<string, string[]> = {}
  let con: Database.Database | null = null
  try {
    con = new Database(p, { readonly: true })
    const ents = con.prepare("SELECT id, canonical_name FROM entity WHERE kind='creator'").all() as any[]
    const linkRows = con.prepare(
      "SELECT e.canonical_name AS cn, COUNT(*) AS n FROM asset_entity ca JOIN entity e ON e.id=ca.entity_id WHERE ca.role='creator' GROUP BY e.id").all() as { cn: string; n: number }[]
    for (const r of linkRows) links[r.cn] = r.n
    for (const e of ents) {
      aliases[e.canonical_name] = (con.prepare("SELECT alias FROM entity_alias WHERE entity_id=?").all(e.id) as any[]).map((a) => a.alias)
    }
  } catch (e) { console.error("ledger open failed", e) }
  finally { con?.close() }
  return { links, aliases }
}

export function invalidateIntegrationCache() { cache.ledgerTs = 0; cache.walkTs = 0; cache.integrated = undefined; walkGeneration++; pendingWalk=undefined; pendingLedger=undefined;invalidatePersonStats() }

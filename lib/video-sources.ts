import fs from 'fs'
import Database from 'better-sqlite3'
import { BASE, MEDIA2 } from './env'
import { chooseSource } from './source-resolution'
import { readJson } from './jsonio'

let sourcesCache: { dbFp: string; overridesRef: any; entries: (readonly [string, string])[] } | null = null
export function readVideoSources(): Map<string,string> {
  const dbPath = `${BASE}/faces.db`
  const overrides = readJson<Record<string,string>>(`${BASE}/video-path-overrides.json`,{})
  let dbFp = ''
  try {
    const s = fs.statSync(dbPath)
    let walFp = ''
    try { const w = fs.statSync(`${dbPath}-wal`); walFp = `${w.mtimeMs}:${w.size}` } catch {}
    dbFp = `${s.mtimeMs}:${s.size}:${s.ino}:${walFp}`
  } catch {}
  if (sourcesCache && dbFp && sourcesCache.dbFp === dbFp && sourcesCache.overridesRef === overrides) {
    return new Map(sourcesCache.entries)
  }
  const db=new Database(dbPath,{readonly:true,fileMustExist:true})
  try{
    const entries: (readonly [string, string])[] = [
      ...(db.prepare('SELECT vid,path FROM videos').all() as {vid:string;path:string}[]).map(r=>[r.vid,r.path] as const),
      ...Object.entries(overrides),
    ]
    if (dbFp) sourcesCache = { dbFp, overridesRef: overrides, entries }
    return new Map(entries)
  }
  finally{db.close()}
}
export function isMediaFile(path:string):boolean {
  try{return fs.statSync(/*turbopackIgnore: true*/ path).isFile()}
  catch(e:any){if(['ENOENT','ENOTDIR'].includes(e.code))return false;throw e}
}
export function videoSource(vid:string,sources:Map<string,string>):string|null {
  return chooseSource(vid,sources.get(vid),MEDIA2,isMediaFile)
}

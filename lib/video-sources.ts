import fs from 'fs'
import Database from 'better-sqlite3'
import { BASE, MEDIA2 } from './env'
import { chooseSource } from './source-resolution'
import { readJson } from './jsonio'

export function readVideoSources(): Map<string,string> {
  const db=new Database(`${BASE}/faces.db`,{readonly:true,fileMustExist:true})
  try{return new Map([...(db.prepare('SELECT vid,path FROM videos').all() as {vid:string;path:string}[]).map(r=>[r.vid,r.path] as const),...Object.entries(readJson<Record<string,string>>(`${BASE}/video-path-overrides.json`,{}))])}
  finally{db.close()}
}
export function isMediaFile(path:string):boolean {
  try{return fs.statSync(/*turbopackIgnore: true*/ path).isFile()}
  catch(e:any){if(['ENOENT','ENOTDIR'].includes(e.code))return false;throw e}
}
export function videoSource(vid:string,sources:Map<string,string>):string|null {
  return chooseSource(vid,sources.get(vid),MEDIA2,isMediaFile)
}

import fs from 'fs'
import { createHash } from 'node:crypto'
import { BASE } from './env'
import { readJson } from './jsonio'
import { healthMatches, type VideoHealthReport } from './video-health-model'

export const HEALTH_F = `${BASE}/video-health.json`
export const HEALTH_REPORTS_F = `${BASE}/video-health-reports.json`
export function loadVideoHealth(): VideoHealthReport { return readJson(HEALTH_F,{records:{}}) }
export function healthScope(report = loadVideoHealth()) {
  let origins:Record<string,{restored_path?:string;restored_by?:string}>|null={}
  try{origins=JSON.parse(fs.readFileSync(`${BASE}/staging-origins.json`,'utf8'));if(!origins||Array.isArray(origins)||typeof origins!=='object')origins=null}catch(e){if((e as NodeJS.ErrnoException).code!=='ENOENT')origins=null}
  const restoration=(path:string)=>Object.entries(origins||{}).filter(([,row])=>row&&row.restored_path===path&&row.restored_by).sort(([a],[b])=>a<b?-1:a>b?1:0).map(([origin,row])=>[origin,row.restored_by])
  const age=report.last_success_at?Date.now()-Date.parse(report.last_success_at):Infinity
  const fresh=age>=0&&age<7*86400_000
  return (vid:string,path:string):boolean=>{
    if(!fresh||!origins||!report.records[vid]?.hidden)return false
    if(process.env.FILE_OPERATIONS_BACKEND==='rust'&&!report.records[vid].fingerprint)return false
    if(report.schema===2&&JSON.stringify(report.records[vid].restorations||[])!==JSON.stringify(restoration(path)))return false
    try {
      const info=fs.statSync(path,{bigint:true})
      if(report.schema===2){
        const current=createHash('sha256').update(JSON.stringify([path,info.dev.toString(),info.ino.toString(),info.size.toString(),info.mtimeNs.toString()])).digest('hex')
        if(report.records[vid].fingerprint!==current)return false
      }
      if(!info.isFile())return report.records[vid].path===path&&report.records[vid].status==='not_file'
      return healthMatches(report.records[vid],path,Number(info.size),info.mtimeNs.toString())
    } catch(e) {
      return ['ENOENT','ENOTDIR'].includes((e as NodeJS.ErrnoException).code||'')&&report.records[vid].path===path&&['missing','not_file'].includes(report.records[vid].status)
    }
  }
}

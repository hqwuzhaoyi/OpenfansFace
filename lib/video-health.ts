import fs from 'fs'
import { BASE } from './env'
import { readJson } from './jsonio'
import { healthMatches, type VideoHealthReport } from './video-health-model'

export const HEALTH_F = `${BASE}/video-health.json`
export const HEALTH_REPORTS_F = `${BASE}/video-health-reports.json`
export function loadVideoHealth(): VideoHealthReport { return readJson(HEALTH_F,{records:{}}) }
export function healthScope(report = loadVideoHealth()) {
  const fresh = !!report.last_success_at && Date.now()-Date.parse(report.last_success_at)<7*86400_000
  return (vid:string,path:string):boolean=>{
    if(!fresh||!report.records[vid]?.hidden)return false
    try {
      const info=fs.statSync(path,{bigint:true})
      if(!info.isFile())return report.records[vid].path===path&&report.records[vid].status==='not_file'
      return healthMatches(report.records[vid],path,Number(info.size),info.mtimeNs.toString())
    } catch(e) {
      return ['ENOENT','ENOTDIR'].includes((e as NodeJS.ErrnoException).code||'')&&report.records[vid].path===path&&['missing','not_file'].includes(report.records[vid].status)
    }
  }
}

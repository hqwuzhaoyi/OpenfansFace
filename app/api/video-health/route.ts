import { NextRequest, NextResponse } from 'next/server'
import { loadVideoHealth } from '@/lib/video-health'
import { nativeMetadataEnabled } from '@/lib/shared-metadata-backend'
import { rustRequest } from '@/lib/file-operation-backend'
import { readVideoSources, videoSource } from '@/lib/video-sources'
import { fingerprint } from '@/lib/person-file-paths'

export async function GET(req:NextRequest) {
  try {
  const vid=req.nextUrl.searchParams.get('vid')
  if(nativeMetadataEnabled())return NextResponse.json(await rustRequest(null,undefined,'v1/video-health'+(vid?'?vid='+encodeURIComponent(vid)+(req.nextUrl.searchParams.get('path')?'&path='+encodeURIComponent(req.nextUrl.searchParams.get('path')!):''):'')))
  if(vid){const path=videoSource(vid,readVideoSources());if(!path)return NextResponse.json({error:'视频不存在'},{status:404});return NextResponse.json({vid,path,fingerprint:fingerprint(path)})}
  const report=loadVideoHealth()
  const fresh=!!report.last_success_at&&Date.now()-Date.parse(report.last_success_at)<7*86400_000
  const rows=Object.values(report.records).filter(r=>!['healthy','unchecked','present'].includes(r.status))
    .map(r=>({...r,hidden:!!r.hidden&&fresh}))
    .sort((a,b)=>Number(!!b.hidden)-Number(!!a.hidden)||a.status.localeCompare(b.status)||String(a.path).localeCompare(String(b.path)))
  const {records,...summary}=report
  return NextResponse.json({...summary,hidden:rows.filter(r=>r.hidden).length,rows,...(!fresh&&report.last_success_at?{error:'检查结果超过 7 天，暂停自动收起，等待下一次成功检查。'}:{})})
  } catch(error) { const e=error as Error&{status?:number};return NextResponse.json({error:e.message},{status:e.status||503}) }
}

import fs from 'node:fs'
import { BASE } from '@/lib/env'
import { randomUUID } from 'node:crypto'
import { nativeMetadataEnabled } from '@/lib/shared-metadata-backend'
import { rustRequest } from '@/lib/file-operation-backend'
import { NextRequest, NextResponse } from 'next/server'
import { readVideoSources, videoSource } from '@/lib/video-sources'
import { acquireReviewLock, loadArchive } from '@/lib/review-store'
import { readJson, writeJson } from '@/lib/jsonio'
import { HEALTH_REPORTS_F } from '@/lib/video-health'
import { validateVids, ReviewError } from '@/lib/review-logic'

export async function POST(req:NextRequest) {
  let release:(()=>void)|undefined
  try {
    const body=await req.json(),[vid]=validateVids([body.vid])
    if(nativeMetadataEnabled())return NextResponse.json(await rustRequest({...body,operation_id:body.operation_id||randomUUID()},undefined,'v1/video-health'))
    if(fs.existsSync(`${BASE}/file-operations-rust/operations.sqlite3`))return NextResponse.json({error:'视频体检已由 Rust 接管，请恢复后端配置'},{status:503})
    const sources=readVideoSources(),archive=loadArchive()
    if(!sources.has(vid)&&!archive.some(v=>v.vid===vid))throw new ReviewError('视频记录不存在',404)
    for(const item of archive)sources.set(item.vid,item.win_dst)
    const current=videoSource(vid,sources)
    if(body.path&&videoSource(String(body.path),new Map())!==current)throw new ReviewError('预览路径与当前视频不一致，暂不记入自动体检',409)
    release=acquireReviewLock()
    const reports=readJson<Record<string,any>>(HEALTH_REPORTS_F,{}),previous=reports[vid]
    if(!previous||Date.now()-Date.parse(previous.last_reported_at)>5*60_000){
      reports[vid]={vid,path:current,last_reported_at:new Date().toISOString(),count:(previous?.count||0)+1,media_error_code:[1,2,3,4].includes(body.code)?body.code:null}
      const recent=Object.entries(reports).sort((a,b)=>b[1].last_reported_at.localeCompare(a[1].last_reported_at)).slice(0,1000)
      writeJson(HEALTH_REPORTS_F,Object.fromEntries(recent))
    }
    return NextResponse.json({ok:true})
  }catch(e:any){return NextResponse.json({error:e.message},{status:e.status||500})}
  finally{release?.()}
}

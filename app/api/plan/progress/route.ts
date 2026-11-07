import { nativeArchiveEnabled, nativeArchive, nativeArchiveProgress } from '@/lib/archive-native-backend'
import fs from 'node:fs'
import path from 'node:path'
import { NextRequest,NextResponse } from 'next/server'
import { BASE } from '@/lib/env'
import { readArchiveRun,saveArchiveRun,validArchiveId } from '@/lib/archive-run'
import { normalizeJob } from '@/lib/home-model'
import { acquireReviewLock } from '@/lib/review-store'
import { loadQueue } from '@/lib/jsonio'
export async function GET(req:NextRequest){
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchiveProgress(req.nextUrl.searchParams.get('id')),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message},{status:error.status||500})}
  try{
    let id=req.nextUrl.searchParams.get('id')
    if(!id){try{id=JSON.parse(fs.readFileSync(path.join(BASE,'jobs/archive.json'),'utf8')).id}catch(e){if((e as NodeJS.ErrnoException).code!=='ENOENT')throw e}}
    if(!id)return NextResponse.json({job:null},{headers:{'Cache-Control':'no-store'}})
    if(!validArchiveId(id))return NextResponse.json({error:'操作编号无效'},{status:400})
    const run=readArchiveRun(id)
    if(!run)return NextResponse.json({job:null},{headers:{'Cache-Control':'no-store'}})
    return NextResponse.json({job:normalizeJob(run.job,Date.now()/1000,run.job.heartbeat_at),result:run.result},{headers:{'Cache-Control':'no-store'}})
  }catch(e){return NextResponse.json({error:(e as Error).message},{status:503})}
}

/** A missing receipt alone cannot disprove a delayed POST. Fence its ID before retrying. */
export async function POST(req:NextRequest){
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('archive-reconcile',await req.json()),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message},{status:error.status||500})}
  let release:ReturnType<typeof acquireReviewLock>|undefined
  try{
    const {id}=await req.json()
    if(!validArchiveId(id))return NextResponse.json({error:'操作编号无效'},{status:400})
    const existing=readArchiveRun(id)
    if(existing)return NextResponse.json({job:existing.job,result:existing.result})
    release=acquireReviewLock()
    const raced=readArchiveRun(id)
    if(raced)return NextResponse.json({job:raced.job,result:raced.result})
    const now=Date.now()/1000
    const result={ok:false,moved:0,remaining:loadQueue().length,errors:['本次请求尚未开始，已阻止迟到请求。请刷新预览后重新归档。']}
    const job={id,kind:'archive' as const,title:'归档同步',status:'error' as const,stage:'未执行',done:0,total:0,unit:'个文件',failed:0,started_at:now,updated_at:now,heartbeat_at:now,finished_at:now,href:'/integration'}
    saveArchiveRun({fingerprint:'cancelled-before-start',job,result})
    return NextResponse.json({job,result})
  }catch(e){return NextResponse.json({error:(e as Error).message},{status:(e as {status?:number}).status||500})}
  finally{release?.()}
}

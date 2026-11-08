import { NextRequest,NextResponse } from 'next/server'
import { nativeArchiveProgress } from '@/lib/archive-native-backend'
import { normalizeJob } from '@/lib/home-model'
import { homeTasks } from '@/lib/home-tasks'
export const runtime='nodejs'
let rebuild:{at:number;running:boolean;msg?:string}|undefined
export async function GET(req:NextRequest){try{
  const data=homeTasks(req.nextUrl.searchParams),url=process.env.CALIBRATOR_URL
  const archive=await nativeArchiveProgress(null)
  data.jobs=data.jobs.filter(job=>job.kind!=='archive')
  if(archive.job&&(!archive.job.finished_at||Date.now()/1000-archive.job.finished_at<86400))data.jobs.push(normalizeJob(archive.job,Date.now()/1000,archive.job.heartbeat_at))
  if(url&&(!rebuild||Date.now()-rebuild.at>5000)){
    try{const res=await fetch(url+'/api/rebuild',{cache:'no-store',signal:AbortSignal.timeout(1200)});if(res.ok){const state=await res.json();rebuild={at:Date.now(),running:state.running===true,msg:state.msg}}}catch{}
  }
  if(rebuild?.running)data.jobs.push({id:'reference-rebuild',kind:'rebuild',title:'人物参考重算',status:Date.now()-rebuild.at>90000?'offline':'running',stage:'重新计算人物归属',done:null,total:null,unit:'',failed:0,started_at:null,updated_at:null,heartbeat_at:rebuild.at/1000,href:'/persons'})
  return NextResponse.json(data,{headers:{'Cache-Control':'no-store'}})
}catch(e){return NextResponse.json({error:(e as Error).message},{status:503})}}

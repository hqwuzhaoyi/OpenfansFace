import { NextRequest, NextResponse } from 'next/server'
import { stagingStats } from '@/lib/integration'
import { selectedPersonFileBackend } from '@/lib/file-operation-routing'
import { stagingList, stagingOperation, stagingStatus } from '@/lib/staging-backend'
const headers={'Cache-Control':'no-store'}
function failure(error:unknown){const e=error as Error&{status?:number;code?:string;path?:string;step?:string};return NextResponse.json({error:e.message||'暂存操作失败',code:e.code,path:e.path,step:e.step},{status:e.status||500,headers})}
export async function GET(req:NextRequest){
  try {
    const id=req.nextUrl.searchParams.get('id')
    if(id)return NextResponse.json(await stagingStatus(id),{headers})
    if(selectedPersonFileBackend('restore-files')==='legacy')return NextResponse.json(stagingStats(),{headers})
    return NextResponse.json(await stagingList(req.nextUrl.searchParams.get('offset')||'0',req.nextUrl.searchParams.get('q')||''),{headers})
  }catch(error){return failure(error)}
}
export async function POST(req:NextRequest){try{const body=await req.json();return NextResponse.json(await stagingOperation(body.mode,body.request,body.version,body.confirmation),{headers})}catch(error){return failure(error)}}

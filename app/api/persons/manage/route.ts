import { NextRequest, NextResponse } from 'next/server'
import { previewPersonFileOperation, applyPersonFileOperation, personFileOperationStatus, reconcilePersonFileOperation } from '@/lib/file-operation-backend'
export async function POST(req:NextRequest){
  try{
    const body=await req.json()
    if(body.mode==='preview')return NextResponse.json(await previewPersonFileOperation(body.request),{headers:{'Cache-Control':'no-store'}})
    if(body.mode==='reconcile')return NextResponse.json(await reconcilePersonFileOperation(body.request,body.version),{headers:{'Cache-Control':'no-store'}})
    if(body.mode==='apply')return NextResponse.json(await applyPersonFileOperation(body.request,body.version),{headers:{'Cache-Control':'no-store'}})
    return NextResponse.json({error:'请选择预览或执行操作'},{status:400})
  }catch(error:any){return NextResponse.json({error:error.message||'文件操作失败，文件未确认变更',code:error.code,path:error.path,step:error.step},{status:error.status||500})}
}

export async function GET(req:NextRequest){
  try{return NextResponse.json(await personFileOperationStatus(req.nextUrl.searchParams.get('id')||''),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message||'操作回执读取失败',code:error.code,path:error.path,step:error.step},{status:error.status||500})}
}

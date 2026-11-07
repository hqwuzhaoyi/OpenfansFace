import { NextRequest, NextResponse } from 'next/server'
import { previewPersonFiles, applyPersonFiles } from '@/lib/person-file-operations'
export async function POST(req:NextRequest){
  try{
    const body=await req.json()
    if(body.mode==='preview')return NextResponse.json(await previewPersonFiles(body.request),{headers:{'Cache-Control':'no-store'}})
    if(body.mode==='apply')return NextResponse.json(await applyPersonFiles(body.request,body.version),{headers:{'Cache-Control':'no-store'}})
    return NextResponse.json({error:'请选择预览或执行操作'},{status:400})
  }catch(error:any){return NextResponse.json({error:error.message||'文件操作失败，文件未确认变更'},{status:error.status||500})}
}

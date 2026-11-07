import { NextRequest, NextResponse } from 'next/server'
import { previewPersonBatchOperation, applyPersonBatchOperation, personBatchOperationProgress, reconcilePersonBatchOperation } from '@/lib/person-batch-backend'

export async function POST(req:NextRequest){
  try{
    const body=await req.json()
    const result=body.mode==='preview'?await previewPersonBatchOperation(body.request):body.mode==='apply'?await applyPersonBatchOperation(body.id,body.version):body.mode==='reconcile'?await reconcilePersonBatchOperation(body.id,body.version):null
    return result?NextResponse.json(result,{headers:{'Cache-Control':'no-store'}}):NextResponse.json({error:'请选择预览或执行操作'},{status:400})
  }catch(error:any){return NextResponse.json({error:error.message||'批量操作失败'},{status:error.status||500})}
}

export async function GET(req:NextRequest){
  try{return NextResponse.json(await personBatchOperationProgress(req.nextUrl.searchParams.get('id')||''),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message||'状态读取失败'},{status:error.status||500})}
}

import { NextRequest,NextResponse } from 'next/server'
import { invalidateIntegrationCacheAsync } from '@/lib/integration'
import { processingSettings,setProcessingConcurrency } from '@/lib/processing-dashboard'
export const runtime='nodejs'
export async function GET(){try{return NextResponse.json(await processingSettings(),{headers:{'Cache-Control':'no-store'}})}catch(e:any){return NextResponse.json({schema_version:1,error:e.message},{status:e.status||503})}}
export async function PUT(req:NextRequest){
  let body:any
  try{body=await req.json()}catch{return NextResponse.json({schema_version:1,error:'请求必须为有效 JSON 对象'},{status:400})}
  if(!body||typeof body!=='object'||Array.isArray(body))return NextResponse.json({schema_version:1,error:'请求必须为 JSON 对象'},{status:400})
  try{
    const result=await setProcessingConcurrency(body.concurrency)
    await invalidateIntegrationCacheAsync()
    return NextResponse.json(result)
  }catch(e:any){return NextResponse.json({schema_version:1,error:e.message},{status:e.status||500})}
}

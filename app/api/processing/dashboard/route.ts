import { NextRequest,NextResponse } from 'next/server'
import { processingDashboard,retryProcessingErrors } from '@/lib/processing-dashboard'
export const runtime='nodejs'
export async function GET(){try{return NextResponse.json(processingDashboard(),{headers:{'Cache-Control':'no-store'}})}catch(e:any){return NextResponse.json({schema_version:1,error:e.message},{status:e.status||503})}}
export async function POST(req:NextRequest){
  let body:any
  try{body=await req.json()}catch{return NextResponse.json({schema_version:1,error:'请求必须为有效 JSON 对象'},{status:400})}
  if(!body||typeof body!=='object'||Array.isArray(body))return NextResponse.json({schema_version:1,error:'请求必须为 JSON 对象'},{status:400})
  try{if(body.action!=='retry')return NextResponse.json({schema_version:1,error:'未知操作'},{status:400});return NextResponse.json(await retryProcessingErrors(body.batch))}catch(e:any){return NextResponse.json({schema_version:1,error:e.message},{status:e.status||500})}
}

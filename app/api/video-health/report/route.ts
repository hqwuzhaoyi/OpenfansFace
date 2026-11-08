import { randomUUID } from 'node:crypto'
import { rustRequest } from '@/lib/file-operation-backend'
import { NextRequest, NextResponse } from 'next/server'
export async function POST(req:NextRequest) {
  try { const body=await req.json(); return NextResponse.json(await rustRequest({...body,operation_id:body.operation_id||randomUUID()},undefined,'v1/video-health')) }
  catch(error:any) { return NextResponse.json({error:error.message,code:error.code},{status:error.status||500}) }
}

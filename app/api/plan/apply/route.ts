import { nativeArchive } from '@/lib/archive-native-backend'
import { NextRequest, NextResponse } from 'next/server'
export async function POST(req:NextRequest) {
  try { return NextResponse.json(await nativeArchive('archive-apply',await req.json()),{headers:{'Cache-Control':'no-store'}}) }
  catch(error:any) { return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500}) }
}

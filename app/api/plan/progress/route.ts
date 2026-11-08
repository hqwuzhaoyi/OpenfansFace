import { nativeArchive, nativeArchiveProgress } from '@/lib/archive-native-backend'
import { NextRequest, NextResponse } from 'next/server'
export async function GET(req:NextRequest) {
  try { return NextResponse.json(await nativeArchiveProgress(req.nextUrl.searchParams.get('id')),{headers:{'Cache-Control':'no-store'}}) }
  catch(error:any) { return NextResponse.json({error:error.message,code:error.code},{status:error.status||503}) }
}
export async function POST(req:NextRequest) {
  try { return NextResponse.json(await nativeArchive('archive-reconcile',await req.json()),{headers:{'Cache-Control':'no-store'}}) }
  catch(error:any) { return NextResponse.json({error:error.message,code:error.code},{status:error.status||503}) }
}

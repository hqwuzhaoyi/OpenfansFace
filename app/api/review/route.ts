import { NextRequest, NextResponse } from 'next/server'
import { nativeArchive,nativeArchiveEnabled } from '@/lib/archive-native-backend'
import { ReviewError, validateVids } from '@/lib/review-logic'

export async function GET(req: NextRequest) {
  try { const params=req.nextUrl.searchParams; return NextResponse.json(await nativeArchive(params.has('status')?'review-status':'review-page',Object.fromEntries(params)),{headers:{'Cache-Control':'no-store'}}) }
  catch (e:any) { return NextResponse.json({error:e.message,code:e.code},{status:e.status || 500}) }
}
export async function POST(req: NextRequest) {
  try {
    const body=await req.json()
    if (!['defer','restore','note'].includes(body.action)) throw new ReviewError('未知操作')
    if (body.action==='note' && (typeof body.note!=='string' || body.note.length>1000)) throw new ReviewError('备注最多 1000 字')
    const vids=validateVids(body.vids)
    return NextResponse.json(await nativeArchive('review-'+body.action,{...body,vids}),{headers:{'Cache-Control':'no-store'}})
  } catch(e:any) { return NextResponse.json({error:e.message,code:e.code},{status:e.status || 500}) }
}

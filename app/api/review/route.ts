import { NextRequest, NextResponse } from 'next/server'
import { processingAction, reviewData } from '@/lib/processing'
import { ReviewError, validateVids } from '@/lib/review-logic'

export async function GET(req: NextRequest) {
  try { return NextResponse.json(reviewData(req.nextUrl.searchParams)) }
  catch (e:any) { return NextResponse.json({error:e.message},{status:e.status || 500}) }
}
export async function POST(req: NextRequest) {
  try {
    const body=await req.json()
    if (!['defer','restore','note'].includes(body.action)) throw new ReviewError('未知操作')
    if (body.action==='note' && (typeof body.note!=='string' || body.note.length>1000)) throw new ReviewError('备注最多 1000 字')
    const vids=validateVids(body.vids)
    processingAction(body.action,vids,body.note)
    return NextResponse.json({ok:true})
  } catch(e:any) { return NextResponse.json({error:e.message},{status:e.status || 500}) }
}

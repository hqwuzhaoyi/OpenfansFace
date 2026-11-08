import { NextRequest, NextResponse } from 'next/server'
import { processingAction, processingOverview } from '@/lib/processing'
import { validateVids, ReviewError } from '@/lib/review-logic'

export async function GET(req: NextRequest) {
  try { return NextResponse.json(await processingOverview(req.nextUrl.searchParams)) }
  catch (e: any) { return NextResponse.json({error:e.message},{status:e.status || 500}) }
}
export async function POST(req: NextRequest) {
  try {
    const body = await req.json()
    if (!['pause','resume','priority','retry','takeover'].includes(body.action)) throw new ReviewError('未知操作')
    const vids = ['pause','resume'].includes(body.action) ? [] : validateVids(body.vids)
    await processingAction(body.action,vids)
    return NextResponse.json({ok:true})
  } catch (e:any) { return NextResponse.json({error:e.message},{status:e.status || 500}) }
}

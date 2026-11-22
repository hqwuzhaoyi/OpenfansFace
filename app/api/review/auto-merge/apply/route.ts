import { nativeAutoWatermarkApply, nativeAutoWatermarkApplyAsync } from '@/lib/archive-native-backend'
import { NextRequest, NextResponse } from 'next/server'

export async function POST(req: NextRequest) {
  try {
    const body = await req.json()
    if (body?.async === true) {
      const { async: _, ...payload } = body
      const result = await nativeAutoWatermarkApplyAsync(payload)
      if (result?.accepted === true && result?.status === 'running') {
        return NextResponse.json(result, { status: 202, headers: { 'Cache-Control': 'no-store' } })
      }
      return NextResponse.json(result, { headers: { 'Cache-Control': 'no-store' } })
    }
    return NextResponse.json(await nativeAutoWatermarkApply(body), { headers: { 'Cache-Control': 'no-store' } })
  } catch (error: any) {
    return NextResponse.json(
      { error: error.message, code: error.code, path: error.path, step: error.step },
      { status: error.status || 500 },
    )
  }
}

import { nativeAutoWatermarkPreview } from '@/lib/archive-native-backend'
import { NextResponse } from 'next/server'

export async function GET() {
  try {
    return NextResponse.json(await nativeAutoWatermarkPreview(), { headers: { 'Cache-Control': 'no-store' } })
  } catch (error: any) {
    return NextResponse.json(
      { error: error.message, code: error.code, path: error.path, step: error.step },
      { status: error.status || 500 },
    )
  }
}

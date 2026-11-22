import { nativeArchiveUnarchive } from '@/lib/archive-native-backend'
import { NextRequest, NextResponse } from 'next/server'

export async function POST(req: NextRequest) {
  try {
    const body = await req.json()
    if (!body || typeof body !== 'object' || typeof body.vid !== 'string' || !body.vid.trim()) {
      return NextResponse.json({ error: '缺少视频编号' }, { status: 400 })
    }
    return NextResponse.json(await nativeArchiveUnarchive(body), { headers: { 'Cache-Control': 'no-store' } })
  } catch (error: any) {
    return NextResponse.json(
      { error: error.message, code: error.code, path: error.path, step: error.step },
      { status: error.status || 500 },
    )
  }
}

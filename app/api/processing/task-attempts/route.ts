import { NextRequest, NextResponse } from 'next/server'
import { ReviewError } from '@/lib/review-logic'
import { nativeJsonRequest } from '@/lib/native-json-transport'
import type { TaskAttemptRecord } from '@/lib/processing-task-model'

export const runtime = 'nodejs'

export interface TaskAttemptsApiResponse {
  ok: boolean
  vid?: string
  attempts: TaskAttemptRecord[]
  error?: string
  schema_version?: number
}

export async function GET(req: NextRequest) {
  const url = req.nextUrl
  const vid = url.searchParams.get('vid') || ''

  if (!vid) {
    return NextResponse.json(
      { schema_version: 1, ok: false, error: '缺少 vid 参数', attempts: [] },
      {
        status: 400,
        headers: { 'Cache-Control': 'no-store' },
      }
    )
  }

  const queryParams = new URLSearchParams()
  queryParams.set('vid', vid)

  try {
    const data = await nativeJsonRequest<{ ok: boolean; vid: string; attempts: TaskAttemptRecord[] }>(
      'v1/agent/task-attempts?' + queryParams.toString(),
      null,
      {
        label: '任务尝试记录读取',
        completion: 'required',
        error: (message, status) => new ReviewError(message, status),
      }
    )

    return NextResponse.json(data, {
      headers: { 'Cache-Control': 'no-store' },
    })
  } catch (e: any) {
    return NextResponse.json(
      {
        schema_version: 1,
        ok: false,
        error: e.message || '任务尝试历史读取失败',
        attempts: [],
      },
      {
        status: e.status || 503,
        headers: { 'Cache-Control': 'no-store' },
      }
    )
  }
}

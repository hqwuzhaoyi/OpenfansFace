import { NextRequest, NextResponse } from 'next/server'
import { invalidateIntegrationCacheAsync } from '@/lib/integration'
import { ReviewError } from '@/lib/review-logic'
import { nativeJsonRequest } from '@/lib/native-json-transport'
import type { NodeControlResponse } from '@/lib/processing-node-model'

export const runtime = 'nodejs'

export async function POST(req: NextRequest) {
  let body: any
  try {
    body = await req.json()
  } catch {
    return NextResponse.json(
      { schema_version: 1, error: '请求体必须为 JSON' },
      { status: 400 }
    )
  }
  if (!body?.node_id || typeof body.node_id !== 'string') {
    return NextResponse.json(
      { schema_version: 1, error: '缺少有效的 node_id' },
      { status: 400 }
    )
  }
  if (!['revoke_acceptance', 'restore_acceptance', 'drain'].includes(body?.action)) {
    return NextResponse.json(
      { schema_version: 1, error: '不支持的操作动作' },
      { status: 400 }
    )
  }
  try {
    const data = await nativeJsonRequest<NodeControlResponse>(
      'v1/agent/nodes/control',
      body,
      {
        label: '节点接单控制',
        completion: 'required',
        error: (message, status) => new ReviewError(message, status),
      }
    )
    await invalidateIntegrationCacheAsync()
    return NextResponse.json(data)
  } catch (e: any) {
    return NextResponse.json(
      { schema_version: 1, error: e.message || '节点控制操作失败' },
      { status: e.status || 500 }
    )
  }
}

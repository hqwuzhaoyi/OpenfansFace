import { NextRequest, NextResponse } from 'next/server'
import { invalidateIntegrationCacheAsync } from '@/lib/integration'
import { parseLockSnapshotJson } from '@/lib/locks-model'

export const dynamic = 'force-dynamic'
export const runtime = 'nodejs'

const NO_STORE_HEADERS = { 'Cache-Control': 'no-store' }
const SSE_HEADERS = {
  'Content-Type': 'text/event-stream; charset=utf-8',
  'Cache-Control': 'no-cache, no-transform',
  Connection: 'keep-alive',
  'X-Accel-Buffering': 'no',
}

const MAX_TRACKED_SETTLED_OPS = 256

const g = globalThis as typeof globalThis & {
  __faceAdminInvalidatedLockOps?: Set<string>
}

function getInvalidatedLockOps(): Set<string> {
  if (!g.__faceAdminInvalidatedLockOps) {
    g.__faceAdminInvalidatedLockOps = new Set()
  }
  return g.__faceAdminInvalidatedLockOps
}

function recordSettledOpForInvalidation(opId: string): boolean {
  if (!opId) return false
  const seen = getInvalidatedLockOps()
  if (seen.has(opId)) return false
  seen.add(opId)
  if (seen.size > MAX_TRACKED_SETTLED_OPS) {
    const oldest = seen.values().next().value
    if (oldest !== undefined) seen.delete(oldest)
  }
  return true
}

function resolveStreamUrl(): URL | null {
  const configured = process.env.FILE_OPERATIONS_RUST_URL
  if (!configured) return null
  try {
    const url = new URL(
      'v1/locks/stream',
      configured.endsWith('/') ? configured : `${configured}/`,
    )
    if (!['http:', 'https:'].includes(url.protocol)) return null
    return url
  } catch {
    return null
  }
}

export async function GET(request: NextRequest) {
  const url = resolveStreamUrl()
  if (!url) {
    return NextResponse.json(
      { error: 'Rust 文件操作服务地址未配置或无效' },
      { status: 503, headers: NO_STORE_HEADERS },
    )
  }

  const headers: Record<string, string> = { Accept: 'text/event-stream' }
  if (process.env.FILE_OPERATIONS_RUST_TOKEN) {
    headers.Authorization = `Bearer ${process.env.FILE_OPERATIONS_RUST_TOKEN}`
  }

  const upstreamController = new AbortController()
  const onAbort = () => upstreamController.abort()
  if (request.signal.aborted) {
    upstreamController.abort()
  } else {
    request.signal.addEventListener('abort', onAbort, { once: true })
  }

  let upstream: Response
  try {
    upstream = await fetch(url, {
      method: 'GET',
      headers,
      cache: 'no-store',
      signal: upstreamController.signal,
    })
  } catch {
    request.signal.removeEventListener('abort', onAbort)
    return NextResponse.json(
      { error: '无法连接锁状态推送流' },
      { status: 503, headers: NO_STORE_HEADERS },
    )
  }

  if (!upstream.ok || !upstream.body) {
    request.signal.removeEventListener('abort', onAbort)
    const body = await upstream.json().catch(() => null)
    return NextResponse.json(
      { error: typeof body?.error === 'string' ? body.error : '锁状态推送流不可用' },
      { status: upstream.ok ? 502 : upstream.status, headers: NO_STORE_HEADERS },
    )
  }

  const upstreamReader = upstream.body.getReader()
  const decoder = new TextDecoder()
  let lineBuffer = ''
  let closed = false

  const cleanup = () => {
    if (closed) return
    closed = true
    request.signal.removeEventListener('abort', onAbort)
    upstreamController.abort()
    void upstreamReader.cancel().catch(() => {})
  }

  const inspectSseLines = async (chunkText: string) => {
    lineBuffer += chunkText
    const lines = lineBuffer.split('\n')
    lineBuffer = lines.pop() ?? ''

    let shouldInvalidate = false
    for (const rawLine of lines) {
      const line = rawLine.endsWith('\r') ? rawLine.slice(0, -1) : rawLine
      if (!line.startsWith('data:')) continue
      const payload = line.slice(5).trim()
      if (!payload) continue
      const snapshot = parseLockSnapshotJson(payload)
      const settled = snapshot?.last_settled
      if (
        settled?.op_id &&
        settled.outcome === 'complete' &&
        recordSettledOpForInvalidation(settled.op_id)
      ) {
        shouldInvalidate = true
      }
    }

    if (shouldInvalidate) {
      try {
        await invalidateIntegrationCacheAsync()
      } catch {
        // Best-effort cache invalidation must never interrupt SSE forwarding.
      }
    }
  }

  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        while (!closed) {
          const { done, value } = await upstreamReader.read()
          if (done) {
            cleanup()
            try {
              controller.close()
            } catch {}
            return
          }
          if (value && value.byteLength > 0) {
            await inspectSseLines(decoder.decode(value, { stream: true }))
            if (!closed) {
              controller.enqueue(value)
            }
            return
          }
        }
      } catch {
        cleanup()
        try {
          controller.close()
        } catch {}
      }
    },
    cancel() {
      cleanup()
    },
  })

  return new Response(stream, { status: 200, headers: SSE_HEADERS })
}

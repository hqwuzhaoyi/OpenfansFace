import { NextRequest, NextResponse } from 'next/server'

export const dynamic = 'force-dynamic'
export const runtime = 'nodejs'

const NO_STORE_HEADERS = { 'Cache-Control': 'no-store' }

function resolveLocksUrl(route: string): URL | null {
  const configured = process.env.FILE_OPERATIONS_RUST_URL
  if (!configured) return null
  try {
    const url = new URL(route, configured.endsWith('/') ? configured : `${configured}/`)
    if (!['http:', 'https:'].includes(url.protocol)) return null
    return url
  } catch {
    return null
  }
}

export async function GET(request: NextRequest) {
  const url = resolveLocksUrl('v1/locks')
  if (!url) {
    return NextResponse.json(
      { error: 'Rust 文件操作服务地址未配置或无效' },
      { status: 503, headers: NO_STORE_HEADERS },
    )
  }

  const headers: Record<string, string> = { Accept: 'application/json' }
  if (process.env.FILE_OPERATIONS_RUST_TOKEN) {
    headers.Authorization = `Bearer ${process.env.FILE_OPERATIONS_RUST_TOKEN}`
  }

  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), 10_000)
  const onAbort = () => controller.abort()
  if (request.signal.aborted) {
    controller.abort()
  } else {
    request.signal.addEventListener('abort', onAbort, { once: true })
  }

  try {
    const upstream = await fetch(url, {
      method: 'GET',
      headers,
      cache: 'no-store',
      signal: controller.signal,
    })
    const body = await upstream.json().catch(() => null)
    if (!upstream.ok) {
      const message =
        typeof body?.error === 'string' && body.error
          ? body.error
          : '锁状态服务返回错误'
      return NextResponse.json(
        { error: message, code: body?.code },
        { status: upstream.status, headers: NO_STORE_HEADERS },
      )
    }
    if (!body || typeof body !== 'object' || Array.isArray(body)) {
      return NextResponse.json(
        { error: '锁状态服务未返回有效快照' },
        { status: 502, headers: NO_STORE_HEADERS },
      )
    }
    return NextResponse.json(body, { status: 200, headers: NO_STORE_HEADERS })
  } catch {
    return NextResponse.json(
      { error: '无法连接锁状态服务' },
      { status: 503, headers: NO_STORE_HEADERS },
    )
  } finally {
    clearTimeout(timeout)
    request.signal.removeEventListener('abort', onAbort)
  }
}

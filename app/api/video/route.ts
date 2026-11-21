import fs from 'fs'
import path from 'path'
import { NextRequest } from 'next/server'
import { fileStream, findMp4Hev1PatchesAsync } from '@/lib/file-stream'
import { MEDIA2 } from '@/lib/env'
import { readVideoSources, videoSource } from '@/lib/video-sources'

let cachedMediaRoot: { raw: string; real: string } | null = null
async function resolveMediaRoot(): Promise<string | null> {
  if (cachedMediaRoot && cachedMediaRoot.raw === MEDIA2) return cachedMediaRoot.real
  try {
    const real = await fs.promises.realpath(/*turbopackIgnore: true*/ MEDIA2)
    cachedMediaRoot = { raw: MEDIA2, real }
    return real
  } catch {
    return null
  }
}

async function handleVideoRequest(req: NextRequest, headOnly: boolean) {
  const input = req.nextUrl.searchParams.get('p') || ''
  if (!input) return new Response('not found', { status: 404 })
  const mediaRoot = await resolveMediaRoot()
  if (!mediaRoot) return new Response('not found', { status: 404 })

  let mapped: string | null = null
  const normalizedInput = input.replace(/\\/g, '/')
  if (
    !normalizedInput.split('/').includes('..') &&
    (normalizedInput.startsWith(mediaRoot + '/') || normalizedInput.startsWith(MEDIA2.replace(/\/$/, '') + '/'))
  ) {
    try {
      const directStat = await fs.promises.stat(/*turbopackIgnore: true*/ normalizedInput)
      if (directStat.isFile()) mapped = normalizedInput
    } catch {}
  }
  if (!mapped) {
    mapped = videoSource(input, readVideoSources())
  }
  if (!mapped) return new Response('not found', { status: 404 })

  let real: string
  try {
    real = await fs.promises.realpath(/*turbopackIgnore: true*/ mapped)
  } catch {
    return new Response('not found', { status: 404 })
  }
  if (!real.startsWith(mediaRoot + '/')) return new Response('forbidden', { status: 403 })
  let st: fs.Stats
  try {
    st = await fs.promises.stat(/*turbopackIgnore: true*/ real)
  } catch {
    return new Response('not found', { status: 404 })
  }
  const size = st.size
  if (!st.isFile() || !size) return new Response('not found', { status: 404 })
  const types: Record<string, string> = {
    '.mp4': 'video/mp4',
    '.m4v': 'video/mp4',
    '.webm': 'video/webm',
    '.mov': 'video/quicktime',
    '.mkv': 'video/x-matroska',
  }
  const baseName = path.basename(real)
  const encodedFileName = encodeURIComponent(baseName).replace(
    /[!'()*]/g,
    ch => `%${ch.charCodeAt(0).toString(16).toUpperCase()}`
  )
  const headers: Record<string, string> = {
    'Content-Type': types[path.extname(real).toLowerCase()] || 'application/octet-stream',
    'Accept-Ranges': 'bytes',
    'Content-Disposition': `inline; filename*=UTF-8''${encodedFileName}`,
  }
  const requested = req.headers.get('range')
  let start = 0
  let end = size - 1
  let status = 200
  if (requested) {
    const m = /^bytes=(\d*)-(\d*)$/.exec(requested)
    if (!m || (!m[1] && !m[2])) {
      return new Response('invalid range', {
        status: 416,
        headers: { 'Content-Range': `bytes */${size}` },
      })
    }
    if (!m[1]) start = Math.max(0, size - Number(m[2]))
    else {
      start = Number(m[1])
      if (m[2]) end = Math.min(Number(m[2]), size - 1)
    }
    if (
      !Number.isSafeInteger(start) ||
      !Number.isSafeInteger(end) ||
      start < 0 ||
      start >= size ||
      end < start
    ) {
      return new Response('invalid range', {
        status: 416,
        headers: { 'Content-Range': `bytes */${size}` },
      })
    }
    headers['Content-Range'] = `bytes ${start}-${end}/${size}`
    status = 206
  }
  headers['Content-Length'] = String(end - start + 1)
  if (headOnly) return new Response(null, { status, headers })
  const patches = await findMp4Hev1PatchesAsync(real, size, st.mtimeMs, start, end)
  return new Response(fileStream(real, { start, end, signal: req.signal, patches }), {
    status,
    headers,
  })
}

export async function GET(req: NextRequest) {
  return handleVideoRequest(req, false)
}

export async function HEAD(req: NextRequest) {
  return handleVideoRequest(req, true)
}

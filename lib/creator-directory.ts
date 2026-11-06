import fs from 'node:fs/promises'
import path from 'node:path'
import { BASE } from './env'
import { readJson, writeJson } from './jsonio'
import { CREATOR_DIRECTORY_DATA, parseCreatorDirectory } from './creator-directory-model'

const cacheFile = path.join(BASE, 'external-profiles', 'nv-pu-sa.json')
const maxAge = 24 * 60 * 60 * 1000
interface Snapshot { fetched_at: string; data: unknown }
type Directory = { profiles: ReturnType<typeof parseCreatorDirectory>; fetched_at: string; stale: boolean; warning?: string }
let pending: Promise<Directory> | undefined
let retryAfter = 0
let lastError = ''

function savedDirectory(): Directory | null {
  const saved = readJson<Snapshot | null>(cacheFile, null)
  if (!saved || !Number.isFinite(Date.parse(saved.fetched_at))) return null
  try {
    const age = Date.now() - Date.parse(saved.fetched_at)
    return { profiles: parseCreatorDirectory(saved.data), fetched_at: saved.fetched_at, stale: age < 0 || age >= maxAge }
  } catch { return null }
}

async function updateDirectory(saved: Directory | null): Promise<Directory> {
  try {
    const response = await fetch(CREATOR_DIRECTORY_DATA, { cache: 'no-store', signal: AbortSignal.timeout(10_000) })
    if (!response.ok) throw new Error(`资料站返回 HTTP ${response.status}`)
    // Bound both declared and streamed size; a broken upstream must not exhaust the app.
    const maxBytes = 10 * 1024 * 1024
    if (Number(response.headers.get('content-length')) > maxBytes) throw new Error('账号资料体积异常')
    const reader = response.body?.getReader()
    if (!reader) throw new Error('账号资料响应为空')
    const chunks: Uint8Array[] = []
    let size = 0
    try {
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        size += value.byteLength
        if (size > maxBytes) throw new Error('账号资料体积异常')
        chunks.push(value)
      }
    } finally { await reader.cancel().catch(() => {}) }
    const data: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    const profiles = parseCreatorDirectory(data), fetched_at = new Date().toISOString()
    await fs.mkdir(path.dirname(cacheFile), { recursive: true })
    writeJson(cacheFile, { fetched_at, data })
    retryAfter = 0; lastError = ''
    return { profiles, fetched_at, stale: false }
  } catch {
    retryAfter = Date.now() + 60_000
    lastError = '外部资料暂时无法更新，请稍后重试'
    if (saved) return { ...saved, stale: true, warning: `${lastError}，当前显示上次缓存。` }
    throw new Error(lastError)
  }
}

/** Separate from library loading; requests send no local names, paths or video data upstream. */
export async function creatorDirectory(): Promise<Directory> {
  const saved = savedDirectory()
  if (saved && !saved.stale) return saved
  if (pending) return pending
  if (Date.now() < retryAfter) {
    if (saved) return { ...saved, stale: true, warning: `${lastError}，当前显示上次缓存。` }
    throw new Error(lastError)
  }
  pending = updateDirectory(saved)
  try { return await pending } finally { pending = undefined }
}

import path from 'node:path'
import { createHash } from 'node:crypto'

export function validateArchiveName(name: unknown): string {
  if (typeof name !== 'string' || name !== name.trim() || !name || Buffer.byteLength(name) > 240 || /[\\/:*?"<>|\x00-\x1f]/.test(name) || name.endsWith('.') || ['.', '..'].includes(name)) {
    throw new Error('文件名无效，请勿包含路径分隔符或特殊字符')
  }
  return name
}

export function alternateName(name: string, vid: string): string {
  const ext = path.extname(name), stem = path.basename(name, ext)
  const suffix = ` (保留两份-${createHash('sha256').update(vid).digest('hex').slice(0, 8)})${ext}`
  let shortened = stem
  while (Buffer.byteLength(shortened + suffix) > 240) shortened = shortened.slice(0, -1)
  return validateArchiveName(shortened + suffix)
}


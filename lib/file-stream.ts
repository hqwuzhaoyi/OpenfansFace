import fs from 'node:fs'
import path from 'node:path'

const patchCache = new Map<string, ReadonlyArray<readonly [number, number]>>()
const HVCC_TAG = Buffer.from('hvcC')

/**
 * Locate `hev1` sample description entries that already contain an `hvcC` configuration box
 * inside the `moov` atom of an MP4/MOV file so they can be streamed as `hvc1` for Apple WebKit / iOS Safari.
 * Never mutates the file on disk and preserves 100% identical byte lengths and offsets.
 */
export function findMp4Hev1Patches(
  file: string,
  size: number,
  mtimeMs: number
): ReadonlyArray<readonly [number, number]> {
  const ext = path.extname(file).toLowerCase()
  if (ext !== '.mp4' && ext !== '.m4v' && ext !== '.mov') return []
  const key = `${file}:${size}:${mtimeMs}`
  const cached = patchCache.get(key)
  if (cached) return cached

  const patches: (readonly [number, number])[] = []
  let fd: number | null = null
  try {
    fd = fs.openSync(file, 'r')
    const hdr = Buffer.alloc(16)
    let pos = 0
    while (pos + 8 <= size) {
      const read = fs.readSync(fd, hdr, 0, Math.min(16, size - pos), pos)
      if (read < 8) break
      let boxSize = hdr.readUInt32BE(0)
      const boxType = hdr.toString('latin1', 4, 8)
      let headerLen = 8
      if (boxSize === 1) {
        if (read < 16) break
        boxSize = Number(hdr.readBigUInt64BE(8))
        headerLen = 16
      } else if (boxSize === 0) {
        boxSize = size - pos
      }
      if (!Number.isSafeInteger(boxSize) || boxSize < headerLen || pos + boxSize > size) break

      if (boxType === 'moov' && boxSize <= 32 * 1024 * 1024) {
        const moov = Buffer.allocUnsafe(boxSize)
        const moovRead = fs.readSync(fd, moov, 0, boxSize, pos)
        if (moovRead === boxSize) {
          let searchFrom = 0
          while (searchFrom < moov.length - 24) {
            const stsdIdx = moov.indexOf('stsd', searchFrom, 'latin1')
            if (stsdIdx < 4 || stsdIdx + 16 > moov.length) break
            const entryPos = stsdIdx + 12
            if (entryPos + 86 <= moov.length) {
              const entrySize = moov.readUInt32BE(entryPos)
              const entryFourCC = moov.toString('latin1', entryPos + 4, entryPos + 8)
              if (
                entryFourCC === 'hev1' &&
                entrySize >= 86 &&
                entryPos + entrySize <= moov.length &&
                moov.subarray(entryPos, entryPos + entrySize).includes(HVCC_TAG)
              ) {
                const fourCCFileOffset = pos + entryPos + 4
                // Patch 'hev1' (68 65 76 31) -> 'hvc1' (68 76 63 31)
                patches.push([fourCCFileOffset + 1, 0x76], [fourCCFileOffset + 2, 0x63])
              }
            }
            searchFrom = stsdIdx + 4
          }
        }
        break
      }
      pos += boxSize
    }
  } catch {
    // Ignore probe errors and stream raw bytes untouched
  } finally {
    if (fd !== null) {
      try {
        fs.closeSync(fd)
      } catch {}
    }
  }

  if (patchCache.size > 500) {
    const oldest = patchCache.keys().next().value
    if (oldest) patchCache.delete(oldest)
  }
  patchCache.set(key, patches)
  return patches
}

/** A cancelled browser must stop the file producer before it can enqueue another chunk. */
export function fileStream(
  file: string,
  options: {
    start: number
    end: number
    signal?: AbortSignal
    patches?: ReadonlyArray<readonly [number, number]>
  }
): ReadableStream<Uint8Array> {
  const source = fs.createReadStream(file, {
    start: options.start,
    end: options.end,
    highWaterMark: 64 * 1024,
  })
  let stopped = false
  let detach = () => {}
  let cursor = options.start
  const patches = options.patches
  return new ReadableStream<Uint8Array>(
    {
      start(controller) {
        source.pause()
        const finish = () => {
          if (stopped) return
          stopped = true
          detach()
          controller.close()
        }
        const abort = () => {
          finish()
          source.destroy()
        }
        detach = () => options.signal?.removeEventListener('abort', abort)
        source.on('data', (chunk: Buffer | string) => {
          if (stopped) return
          const buf = typeof chunk === 'string' ? Buffer.from(chunk) : Buffer.from(chunk)
          const chunkStart = cursor
          const chunkEnd = chunkStart + buf.length
          cursor = chunkEnd
          if (patches && patches.length > 0) {
            for (const [patchOffset, patchByte] of patches) {
              if (patchOffset >= chunkStart && patchOffset < chunkEnd) {
                buf[patchOffset - chunkStart] = patchByte
              }
            }
          }
          controller.enqueue(buf)
          if ((controller.desiredSize ?? 0) <= 0) source.pause()
        })
        source.on('error', error => {
          if (stopped) return
          stopped = true
          detach()
          controller.error(error)
        })
        source.once('end', finish)
        source.once('close', () => {
          finish()
          detach()
        })
        if (options.signal?.aborted) abort()
        else options.signal?.addEventListener('abort', abort, { once: true })
      },
      pull() {
        if (!stopped) source.resume()
      },
      cancel() {
        stopped = true
        detach()
        source.destroy()
      },
    },
    { highWaterMark: 64 * 1024, size: chunk => chunk.byteLength }
  )
}

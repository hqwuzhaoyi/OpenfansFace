import net from 'node:net'

type RespValue = string | number | null | RespValue[]

type PendingCommand = {
  resolve: (value: RespValue) => void
  reject: (err: Error) => void
  timer: ReturnType<typeof setTimeout>
}

interface ParsedConfig {
  host: string
  port: number
  username?: string
  password?: string
  db: number
}

const KEY_PREFIX = 'face-admin:'
const CONNECT_TIMEOUT_MS = 350
const COMMAND_TIMEOUT_MS = 800
const COOLDOWN_MS = 5000

function parseRedisUrl(): ParsedConfig | null {
  const raw = process.env.REDIS_URL?.trim()
  if (!raw) return null
  try {
    const u = new URL(raw)
    if (u.protocol !== 'redis:') return null
    const dbRaw = u.pathname.replace(/^\/+/, '')
    const db = dbRaw ? Number(dbRaw) : 0
    return {
      host: u.hostname || '127.0.0.1',
      port: u.port ? Number(u.port) : 6379,
      username: u.username ? decodeURIComponent(u.username) : undefined,
      password: u.password
        ? decodeURIComponent(u.password)
        : process.env.REDIS_PASSWORD || undefined,
      db: Number.isInteger(db) && db >= 0 ? db : 0,
    }
  } catch {
    return null
  }
}

function encodeCommand(args: (string | number)[]): Buffer {
  const chunks: Buffer[] = [Buffer.from(`*${args.length}\r\n`, 'ascii')]
  for (const arg of args) {
    const buf = Buffer.from(String(arg), 'utf8')
    chunks.push(Buffer.from(`$${buf.length}\r\n`, 'ascii'), buf, Buffer.from('\r\n', 'ascii'))
  }
  return Buffer.concat(chunks)
}

function parseResp(buf: Buffer, offset = 0): { value: RespValue; nextOffset: number } | null {
  if (offset >= buf.length) return null
  const prefix = String.fromCharCode(buf[offset])
  const lineEnd = buf.indexOf('\r\n', offset + 1)
  if (lineEnd === -1) return null
  const line = buf.toString('utf8', offset + 1, lineEnd)

  if (prefix === '+') return { value: line, nextOffset: lineEnd + 2 }
  if (prefix === '-') {
    throw Object.assign(new Error(line), { isRedisError: true, nextOffset: lineEnd + 2 })
  }
  if (prefix === ':') return { value: Number(line), nextOffset: lineEnd + 2 }
  if (prefix === '$') {
    const len = Number(line)
    if (len === -1) return { value: null, nextOffset: lineEnd + 2 }
    const dataStart = lineEnd + 2
    const dataEnd = dataStart + len
    if (dataEnd + 2 > buf.length) return null
    return { value: buf.toString('utf8', dataStart, dataEnd), nextOffset: dataEnd + 2 }
  }
  if (prefix === '*') {
    const count = Number(line)
    if (count === -1) return { value: null, nextOffset: lineEnd + 2 }
    let cur = lineEnd + 2
    const items: RespValue[] = []
    for (let i = 0; i < count; i++) {
      const parsed = parseResp(buf, cur)
      if (!parsed) return null
      items.push(parsed.value)
      cur = parsed.nextOffset
    }
    return { value: items, nextOffset: cur }
  }
  throw new Error(`Unexpected RESP prefix: ${prefix}`)
}

class RedisClient {
  private socket: net.Socket | null = null
  private readyPromise: Promise<void> | null = null
  private queue: PendingCommand[] = []
  private rxBuf: Buffer<ArrayBufferLike> = Buffer.alloc(0)
  private disabledUntil = 0
  private lastUrl = ''

  private reset(err?: Error) {
    if (this.socket) {
      this.socket.destroy()
      this.socket = null
    }
    this.readyPromise = null
    this.rxBuf = Buffer.alloc(0)
    while (this.queue.length) {
      const item = this.queue.shift()!
      clearTimeout(item.timer)
      item.reject(err ?? new Error('Redis connection closed'))
    }
  }

  private async ensureConnected(cfg: ParsedConfig): Promise<net.Socket> {
    const urlKey = `${cfg.host}:${cfg.port}/${cfg.db}:${cfg.username ?? ''}:${cfg.password ?? ''}`
    if (this.lastUrl !== urlKey) {
      this.reset()
      this.lastUrl = urlKey
      this.disabledUntil = 0
    }
    if (Date.now() < this.disabledUntil) {
      throw new Error('Redis in cooldown')
    }
    if (this.socket && this.readyPromise) {
      await this.readyPromise
      if (this.socket) return this.socket
    }

    const socket = net.createConnection({ host: cfg.host, port: cfg.port })
    socket.setNoDelay(true)
    this.socket = socket
    this.rxBuf = Buffer.alloc(0)

    socket.on('data', (chunk: Buffer) => {
      this.rxBuf = Buffer.concat([this.rxBuf, chunk])
      while (this.queue.length > 0) {
        try {
          const parsed = parseResp(this.rxBuf, 0)
          if (!parsed) break
          this.rxBuf = this.rxBuf.subarray(parsed.nextOffset)
          const item = this.queue.shift()!
          clearTimeout(item.timer)
          item.resolve(parsed.value)
        } catch (err: any) {
          if (err?.isRedisError && typeof err.nextOffset === 'number') {
            this.rxBuf = this.rxBuf.subarray(err.nextOffset)
            const item = this.queue.shift()
            if (item) {
              clearTimeout(item.timer)
              item.reject(new Error(err.message))
            }
          } else {
            this.disabledUntil = Date.now() + COOLDOWN_MS
            this.reset(err instanceof Error ? err : new Error(String(err)))
            break
          }
        }
      }
    })

    const onSocketFail = (err?: Error) => {
      this.disabledUntil = Date.now() + COOLDOWN_MS
      this.reset(err ?? new Error('Redis socket error'))
    }
    socket.on('error', onSocketFail)
    socket.on('close', () => this.reset())

    this.readyPromise = new Promise<void>((resolve, reject) => {
      const connectTimer = setTimeout(() => {
        this.disabledUntil = Date.now() + COOLDOWN_MS
        const err = new Error('Redis connect timeout')
        this.reset(err)
        reject(err)
      }, CONNECT_TIMEOUT_MS)

      socket.once('connect', async () => {
        clearTimeout(connectTimer)
        try {
          if (cfg.password) {
            const authArgs = cfg.username
              ? ['AUTH', cfg.username, cfg.password]
              : ['AUTH', cfg.password]
            await this.sendOnSocket(socket, authArgs)
          }
          if (cfg.db > 0) {
            await this.sendOnSocket(socket, ['SELECT', cfg.db])
          }
          resolve()
        } catch (err) {
          this.disabledUntil = Date.now() + COOLDOWN_MS
          this.reset(err instanceof Error ? err : new Error(String(err)))
          reject(err)
        }
      })

      socket.once('error', (err) => {
        clearTimeout(connectTimer)
        reject(err)
      })
    })

    await this.readyPromise
    return socket
  }

  private sendOnSocket(socket: net.Socket, args: (string | number)[]): Promise<RespValue> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const idx = this.queue.findIndex((q) => q.timer === timer)
        if (idx >= 0) this.queue.splice(idx, 1)
        this.disabledUntil = Date.now() + COOLDOWN_MS
        this.reset(new Error('Redis command timeout'))
        reject(new Error('Redis command timeout'))
      }, COMMAND_TIMEOUT_MS)
      this.queue.push({ resolve, reject, timer })
      socket.write(encodeCommand(args))
    })
  }

  async exec(args: (string | number)[]): Promise<RespValue> {
    const cfg = parseRedisUrl()
    if (!cfg) throw new Error('REDIS_URL not configured')
    const socket = await this.ensureConnected(cfg)
    return this.sendOnSocket(socket, args)
  }
}

const g = globalThis as typeof globalThis & {
  __faceAdminRedis?: RedisClient
  __faceAdminRedisEpoch?: number
  __faceAdminRedisInFlight?: Map<string, Promise<unknown>>
}

function getClient() {
  if (!g.__faceAdminRedis) g.__faceAdminRedis = new RedisClient()
  return g.__faceAdminRedis
}

function getInFlight() {
  if (!g.__faceAdminRedisInFlight) g.__faceAdminRedisInFlight = new Map()
  return g.__faceAdminRedisInFlight
}

export function isRedisConfigured(): boolean {
  return !!parseRedisUrl()
}

export function getRedisCacheEpoch(): number {
  return g.__faceAdminRedisEpoch ?? 0
}

export function bumpRedisCacheEpoch(): number {
  const next = (g.__faceAdminRedisEpoch ?? 0) + 1
  g.__faceAdminRedisEpoch = next
  getInFlight().clear()
  return next
}

export async function redisGetJson<T>(key: string): Promise<T | undefined> {
  if (!isRedisConfigured()) return undefined
  try {
    const raw = await getClient().exec(['GET', `${KEY_PREFIX}${key}`])
    if (typeof raw !== 'string') return undefined
    return JSON.parse(raw) as T
  } catch {
    return undefined
  }
}

export async function redisSetJson(key: string, value: unknown, ttlSeconds: number): Promise<void> {
  if (!isRedisConfigured()) return
  try {
    const ttl = Math.max(1, Math.floor(ttlSeconds))
    await getClient().exec(['SET', `${KEY_PREFIX}${key}`, JSON.stringify(value), 'EX', ttl])
  } catch {
    // Graceful cache degradation on Redis error
  }
}

export async function redisDelPrefix(prefix = ''): Promise<void> {
  if (!isRedisConfigured()) return
  try {
    const pattern = `${KEY_PREFIX}${prefix}*`
    let cursor = '0'
    let iterations = 0
    do {
      const reply = await getClient().exec(['SCAN', cursor, 'MATCH', pattern, 'COUNT', 200])
      if (!Array.isArray(reply) || reply.length < 2) break
      cursor = String(reply[0] ?? '0')
      const keys = Array.isArray(reply[1])
        ? reply[1].filter((k): k is string => typeof k === 'string' && k.length > 0)
        : []
      if (keys.length > 0) {
        await getClient().exec(['UNLINK', ...keys])
      }
      iterations++
    } while (cursor !== '0' && iterations < 50)
  } catch {
    // Best-effort invalidation
  }
}

export async function cachedRedisJson<T>(
  key: string,
  ttlSeconds: number,
  loader: () => Promise<T>,
  shouldCache: (value: T) => boolean = () => true,
): Promise<T> {
  if (!isRedisConfigured()) {
    return loader()
  }
  const hit = await redisGetJson<T>(key)
  if (hit !== undefined) {
    return hit
  }

  const inFlight = getInFlight()
  const existing = inFlight.get(key) as Promise<T> | undefined
  if (existing) {
    return existing
  }

  const epochAtStart = getRedisCacheEpoch()
  const work = (async () => {
    const value = await loader()
    if (epochAtStart === getRedisCacheEpoch() && shouldCache(value)) {
      await redisSetJson(key, value, ttlSeconds)
    }
    return value
  })()

  inFlight.set(key, work)
  try {
    return await work
  } finally {
    if (inFlight.get(key) === work) {
      inFlight.delete(key)
    }
  }
}

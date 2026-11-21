import { NextRequest, NextResponse } from 'next/server'
import { nativeJsonRequest } from '@/lib/native-json-transport'
import { cachedRedisJson, getRedisCacheEpoch, isRedisConfigured, redisSetJson } from '@/lib/redis'

let pending: Promise<any> | undefined
let cached: { at: number; epoch: number; value: any } | undefined

function isCacheableInventory(value: any): boolean {
  return Boolean(value && typeof value === 'object' && !value.error && Array.isArray(value.assets))
}

async function loadInventory() {
  return nativeJsonRequest<{ assets: any[] }>('v1/metadata', {
    command: 'processing-inventory',
    body: {},
  }, {
    label: '媒体账本快照',
    completion: 'required',
    error: (message, status) => Object.assign(new Error(message), { status }),
  })
}

export async function GET(req: NextRequest) {
  try {
    const params = req.nextUrl.searchParams
    const refresh = params.get('refresh')
    const force = params.get('force')
    const bypass = refresh === '1' || refresh === 'true' || force === '1' || force === 'true'
    const epoch = getRedisCacheEpoch()

    if (bypass) {
      const value = await loadInventory()
      if (isCacheableInventory(value) && epoch === getRedisCacheEpoch()) {
        cached = { at: Date.now(), epoch, value }
        await redisSetJson('processing:inventory', value, 30)
      }
      return NextResponse.json(value)
    }

    if (!isRedisConfigured()) {
      if (cached && cached.epoch === epoch && Date.now() - cached.at < 30_000) {
        return NextResponse.json(cached.value)
      }
      if (!pending) {
        pending = (async () => {
          const value = await loadInventory()
          if (isCacheableInventory(value) && epoch === getRedisCacheEpoch()) {
            cached = { at: Date.now(), epoch, value }
          }
          return value
        })().finally(() => { pending = undefined })
      }
      return NextResponse.json(await pending)
    }

    return NextResponse.json(await cachedRedisJson('processing:inventory', 30, loadInventory, isCacheableInventory))
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 503 }) }
}

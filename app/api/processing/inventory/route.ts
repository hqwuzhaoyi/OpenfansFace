import { NextResponse } from 'next/server'
import { nativeJsonRequest } from '@/lib/native-json-transport'

let pending: Promise<any> | undefined
let cached: { at: number; value: any } | undefined
export async function GET() {
  try {
    if (cached && Date.now() - cached.at < 30_000) return NextResponse.json(cached.value)
    if (!pending) pending = (async () => {
      const value = await nativeJsonRequest<{ assets: any[] }>('v1/metadata', {
        command: 'processing-inventory',
        body: {},
      }, {
        label: '媒体账本快照',
        completion: 'required',
        error: (message, status) => Object.assign(new Error(message), { status }),
      })
      cached = { at: Date.now(), value }
      return value
    })().finally(() => { pending = undefined })
    return NextResponse.json(await pending)
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 503 }) }
}

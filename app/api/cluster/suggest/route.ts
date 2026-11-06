import { NextRequest, NextResponse } from "next/server"

const cal = () => process.env.CALIBRATOR_URL || "http://127.0.0.1:8921"

export async function GET(req: NextRequest) {
  const cid = req.nextUrl.searchParams.get("cid")
  if (!cid) return NextResponse.json({ error: "cid required" }, { status: 400 })
  try {
    const r = await fetch(`${cal()}/api/cluster/suggest?cid=${encodeURIComponent(cid)}`, {
      signal: AbortSignal.timeout(15000),
    })
    const d = await r.json()
    return NextResponse.json(d, { status: r.ok ? 200 : 500 })
  } catch (e: any) {
    return NextResponse.json({ error: String(e?.message || e) }, { status: 502 })
  }
}

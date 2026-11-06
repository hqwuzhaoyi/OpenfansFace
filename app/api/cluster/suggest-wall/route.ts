import { NextRequest, NextResponse } from "next/server"

const cal = () => process.env.CALIBRATOR_URL || "http://127.0.0.1:8921"

// 批量：高置信簇的 top1 推荐（sim≥0.58 且 top1−top2≥0.03），卡片墙一键直选用
export async function GET() {
  try {
    const r = await fetch(`${cal()}/api/cluster/suggest_wall`, {
      signal: AbortSignal.timeout(60000),
    })
    const d = await r.json()
    return NextResponse.json(d, { status: r.ok ? 200 : 500 })
  } catch (e: any) {
    return NextResponse.json({ error: String(e?.message || e) }, { status: 502 })
  }
}

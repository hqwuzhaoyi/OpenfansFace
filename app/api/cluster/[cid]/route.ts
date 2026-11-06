import { NextRequest, NextResponse } from "next/server"
import { pendingClusters, reviewScope } from "@/lib/review-scope"
import path from "path"
import { thumbUrl } from "@/lib/db"


export async function GET(_req: NextRequest, ctx: { params: Promise<{ cid: string }> }) {
  const { cid } = await ctx.params
  const scope=reviewScope()
  for (const c of pendingClusters(scope,cid)) {
    if (c.cid === cid) return NextResponse.json({
      cid: c.cid, total_gb: c.total_gb,
      vids: c.vids.map((v: any) => ({
        vid: v.vid, path: scope.source(v.vid), name: path.basename(scope.source(v.vid)||v.name), size_gb: +(v.size / 2 ** 30).toFixed(2),
        duration: v.duration, thumb: thumbUrl(v.thumb), conf: v.conf || "unknown",
      })),
    })
  }
  return NextResponse.json({ error: "not found" }, { status: 404 })
}

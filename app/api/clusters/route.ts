import { NextResponse } from "next/server"
import { pendingClusters } from "@/lib/review-scope"
import { thumbUrl } from "@/lib/db"


export async function GET() {
  const cs = pendingClusters()
  return NextResponse.json(cs.map((c) => ({
    cid: c.cid, n_videos: c.n_videos, total_gb: c.total_gb,
    rep_thumb: thumbUrl(c.rep_thumb), collections: c.collections, weak_count: c.weak_count,
  })))
}

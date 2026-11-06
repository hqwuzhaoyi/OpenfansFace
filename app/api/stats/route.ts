import { pendingClusters as loadPendingClusters, reviewScope } from "@/lib/review-scope"
import { NextResponse } from "next/server"
import fs from "fs"
import { loadReport, loadClaims, loadQueue, integratedStats, stagingStats } from "@/lib/integration"
import { MEDIA2 } from "@/lib/env"
import { loadDecisions, resolvedVids } from "@/lib/review-store"

declare global { var REBUILD: any }
const REBUILD: any = globalThis.REBUILD ?? { running: false, done_at: null, ok: null, msg: "" }

export async function GET() {
  const r = loadReport()
  const st = r.stats ?? {}
  const creators = integratedStats()
  const staging = stagingStats()
  let disk = null
  try {
    const s = fs.statfsSync(/*turbopackIgnore: true*/ MEDIA2)
    const total = s.blocks * s.bsize, free = s.bavail * s.bsize
    disk = {
      free_gb: +(free / 2 ** 30).toFixed(1), total_gb: +(total / 2 ** 30).toFixed(1),
      used_pct: +(((total - free) / total) * 100).toFixed(1),
    }
  } catch { }
  const dec = loadDecisions(), scope=reviewScope()
  const handled = new Set([...resolvedVids(), ...dec.approved, ...dec.rejected])
  const band = loadClaims().filter((c) => c.s >= 0.5 && c.s < 0.58 && !handled.has(c.vid) && scope.eligible(c.vid))
  const pendingClusters = loadPendingClusters(scope)
  let mytgN = st.scanned || 0
  const { path: _stagingPath, ...stagingParity } = staging
  return NextResponse.json({
    funnel: {
      scanned: st.scanned || 0,
      claimed: st.claimed_n || 0, claimed_gb: st.claimed_gb || 0,
      integrated_files: Object.values(creators).reduce((a, v) => a + v.files, 0),
      integrated_gb: +Object.values(creators).reduce((a, v) => a + v.gb, 0).toFixed(1),
      unknown_clusters: pendingClusters.length,
      unknown_n: pendingClusters.reduce((n,c)=>n+c.n_videos,0), unknown_gb: +pendingClusters.reduce((n,c)=>n+c.total_gb,0).toFixed(1),
      mytg_remaining: mytgN,
    },
    creators_dirs: Object.keys(creators).length,
    pending_review: band.length,
    queue_len: loadQueue().length,
    staging: stagingParity,
    disk,
    rebuild: REBUILD,
  })
}

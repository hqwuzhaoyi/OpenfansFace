import path from "path"
import { reviewScope } from "@/lib/review-scope"
import { NextRequest, NextResponse } from "next/server"
import Database from "better-sqlite3"
import { loadClaims, loadDecisions } from "@/lib/integration"
import { BASE } from "@/lib/env"
import { resolvedVids } from "@/lib/review-store"
import { thumbUrl } from "@/lib/db"

export async function GET(req: NextRequest) {
  const limit = Math.max(1, Math.min(60, Number(req.nextUrl.searchParams.get("limit")) || 8))
  let offset = Math.max(0, Number(req.nextUrl.searchParams.get("offset")) || 0)
  const dec = loadDecisions(), scope=reviewScope()
  const decided = new Set([...dec.approved, ...dec.rejected, ...resolvedVids()])
  const rows = loadClaims().filter((c) => c.s >= 0.5 && c.s < 0.58 && !decided.has(c.vid) && scope.eligible(c.vid)).sort((a, b) => b.s - a.s)
  const focus = req.nextUrl.searchParams.get("vid")
  const index = focus ? rows.findIndex(c => c.vid === focus) : -1
  offset = index >= 0 ? Math.floor(index / limit) * limit : Math.min(offset, Math.max(0, Math.ceil(rows.length / limit) - 1) * limit)
  const page = rows.slice(offset, offset + limit)
  const con = new Database(`${BASE}/faces.db`, { readonly: true })
  try {
    const out = page.map((c) => {
      const th = con.prepare("SELECT thumb FROM faces WHERE vid=? ORDER BY score DESC LIMIT 1").get(c.vid) as any
      return { vid: c.vid, path: scope.source(c.vid)||c.vid, file_name: path.basename(scope.source(c.vid)||c.vid), name: c.name, score: +c.s.toFixed(3), n_faces: c.n, zone: c.zone ?? "", thumb: thumbUrl(th?.thumb) }
    })
    return NextResponse.json({ total: rows.length, offset, claims: out, decided: { approved: dec.approved.length, rejected: dec.rejected.length } })
  } finally { con.close() }
}

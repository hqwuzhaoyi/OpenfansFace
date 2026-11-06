import { NextRequest, NextResponse } from "next/server"
import fs from "fs"
import path from "path"
import { loadClusters } from "@/lib/integration"
import { MEDIA2, STAGE } from "@/lib/env"
import { readVideoSources, videoSource, isMediaFile } from "@/lib/video-sources"

export async function POST(req: NextRequest) {
  const { vids = [] } = await req.json()
  fs.mkdirSync(/*turbopackIgnore: true*/ STAGE, { recursive: true })
  const clusters = loadClusters(), sources=readVideoSources()
  const byVid = new Map<string, any>()
  for (const c of clusters) for (const v of c.vids || []) byVid.set(v.vid, v)
  let moved = 0, freed = 0
  const errors: string[] = []
  for (const vid of vids) {
    const row = byVid.get(vid)
    if (!row) { errors.push(`not found: ${vid.slice(0, 40)}`); continue }
    const src = videoSource(vid,sources)
    if (!src || !isMediaFile(src)) { errors.push(`missing: ${path.basename(src || vid).slice(0, 40)}`); continue }
    const stem = path.basename(src)
    let dst = path.join(STAGE, stem)
    let i = 1
    while (fs.existsSync(/*turbopackIgnore: true*/ dst)) {
      if (i > 1) {
        const m = dst.match(/(.*)__\d+(\.\w+)?$/)
        const base = m ? m[1] : stem
        const ext = (stem.match(/\.\w+$/) || [""])[0]
        dst = path.join(STAGE, `${base}__${i}${ext}`)
      } else {
        const ext = (stem.match(/\.\w+$/) || [""])[0]
        dst = path.join(STAGE, `${stem.replace(/\.\w+$/, "")}__${i}${ext}`)
      }
      i++
    }
    try { fs.renameSync(/*turbopackIgnore: true*/ src, dst); moved++; freed += row.size || 0 }
    catch (e: any) { errors.push(`${stem.slice(0, 30)}: ${e.message?.slice(0, 60)}`) }
  }
  return NextResponse.json({ moved, freed_gb: +(freed / 2 ** 30).toFixed(1), errors })
}

import fs from "fs"
import { NextRequest, NextResponse } from "next/server"

export async function GET(req: NextRequest) {
  const p = req.nextUrl.searchParams.get("p")
  if (!p) return NextResponse.json({ error: "p required" }, { status: 400 })
  let st: fs.Stats | null = null
  try { st = fs.statSync(/*turbopackIgnore: true*/ p) } catch { }
  if (!st) return NextResponse.json({ error: "not found" }, { status: 404 })
  const buf = fs.readFileSync(/*turbopackIgnore: true*/ p)
  return new NextResponse(buf, { headers: { "Content-Type": "image/jpeg", "Cache-Control": "public, max-age=86400" } })
}

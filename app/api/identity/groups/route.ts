import { NextRequest } from "next/server"

const CAL = () => process.env.CALIBRATOR_URL

export async function GET(req: NextRequest) {
  const cal = CAL()
  if (!cal) return Response.json({ error: "calibrator not configured" }, { status: 503 })
  const name = req.nextUrl.searchParams.get("name")
  if (!name) return Response.json({ error: "name required" }, { status: 400 })
  const r = await fetch(`${cal}/api/identity/groups?name=${encodeURIComponent(name)}`)
  return new Response(r.body, { status: r.status, headers: { "Content-Type": "application/json" } })
}

const CAL = () => process.env.CALIBRATOR_URL

export async function GET() {
  const cal = CAL()
  if (!cal) return Response.json({ error: "calibrator not configured" }, { status: 503 })
  const r = await fetch(`${cal}/api/rebuild`)
  return new Response(r.body, { status: r.status, headers: { "Content-Type": "application/json" } })
}

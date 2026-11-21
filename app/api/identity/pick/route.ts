import { invalidateIntegrationCacheAsync } from "@/lib/integration"

const CAL = () => process.env.CALIBRATOR_URL

export async function POST(req: Request) {
  const cal = CAL()
  if (!cal) return Response.json({ error: "calibrator not configured" }, { status: 503 })
  const body = await req.text()
  const r = await fetch(`${cal}/api/identity/pick`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  })
  if (r.ok) {
    await invalidateIntegrationCacheAsync()
  }
  return new Response(r.body, { status: r.status, headers: { "Content-Type": "application/json" } })
}

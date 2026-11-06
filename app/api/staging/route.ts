import { NextResponse } from "next/server"
import { stagingStats } from "@/lib/integration"

export async function GET() {
  return NextResponse.json(stagingStats())
}

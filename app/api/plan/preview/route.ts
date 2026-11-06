import { NextResponse } from 'next/server'
import { buildPlan } from '@/lib/review-plan'
import { loadArchive } from '@/lib/review-store'
export async function GET() {
  return NextResponse.json({ ...buildPlan(), history: loadArchive().slice().reverse().slice(0, 200) })
}

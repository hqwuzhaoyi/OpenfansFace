import { NextResponse } from 'next/server'
import { unclassifiedVideos } from '@/lib/unclassified'

export async function GET() {
  return NextResponse.json(unclassifiedVideos())
}

import { NextResponse } from 'next/server'
import { homeMetrics } from '@/lib/home-metrics'
export const runtime='nodejs'
export async function GET(){try{return NextResponse.json(await homeMetrics(),{headers:{'Cache-Control':'no-store'}})}catch(e){return NextResponse.json({error:(e as Error).message},{status:503})}}

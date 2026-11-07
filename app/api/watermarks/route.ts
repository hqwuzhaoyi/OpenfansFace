import { nativeMetadataEnabled, nativeMetadata } from '@/lib/shared-metadata-backend'
import fs from 'fs'
import { NextRequest, NextResponse } from 'next/server'
import { acquireReviewLock } from '@/lib/review-store'
import { validateCreator, validateVids, ReviewError } from '@/lib/review-logic'
import { writeJson } from '@/lib/jsonio'
import { loadWatermarkMappings, requestWatermarkScan, watermarkData, watermarkFile, WATERMARK_DIR } from '@/lib/watermarks'
import { watermarkRuleChanged } from '@/lib/processing'
import { BASE } from '@/lib/env'
import path from 'path'

export const runtime = 'nodejs'
export async function GET() {
  try { return NextResponse.json(watermarkData()) }
  catch (e: any) { return NextResponse.json({ error: e.message, code:e.code, operation_id:e.operation_id }, { status: e.status || 500 }) }
}
export async function POST() { return NextResponse.json({error:'旧水印扫描入口已退役，请在处理操作台查看或重试任务'},{status:410}) }
export async function PUT(req: NextRequest) {
  let release: (() => void) | undefined
  try {
    const body = await req.json()
    if (typeof body.key !== 'string' || !/^[a-f0-9]{64}$/.test(body.key) || !['person', 'source', 'remove'].includes(body.kind)) throw new ReviewError('水印映射参数无效')
    const creator = body.kind === 'person' ? validateCreator(body.creator) : undefined
    {
      const evidence=body.kind==='remove'?undefined:watermarkData().videos.find(v=>v.watermark?.candidates.some(c=>c.key===body.key))
      const candidate=evidence?.watermark?.candidates.find(c=>c.key===body.key)
      if(body.kind!=='remove'&&!candidate)throw new ReviewError('水印证据已变化，请刷新后重新确认',409)
      const result=await nativeMetadata('watermark-set',{name:creator,key:body.key,kind:body.kind,candidate,evidence_vid:evidence?.vid})
      // Native receipt includes selected-task requeue and in-flight attempt fencing.
      return NextResponse.json(result)
    }
  } catch (e: any) { return NextResponse.json({ error: e.message, code:e.code, operation_id:e.operation_id }, { status: e.status || 500 }) }
  finally { release?.() }
}

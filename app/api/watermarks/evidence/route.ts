import fs from 'fs'
import {acceptedAttemptFile} from '@/lib/processing-artifacts'
import path from 'path'
import { NextRequest, NextResponse } from 'next/server'
import { loadWatermarkRecords, watermarkState, WATERMARK_DIR } from '@/lib/watermarks'
import { readVideoSources, videoSource } from '@/lib/video-sources'
import { watermarkMatches } from '@/lib/watermark-model'

export async function GET(req: NextRequest) {
  try {
    const vid = req.nextUrl.searchParams.get('vid'), key = req.nextUrl.searchParams.get('key')
    if (!vid || !key) return new NextResponse(null, { status: 404 })
    const filePath = videoSource(vid, readVideoSources())
    if (!filePath) return new NextResponse(null, { status: 404 })
    const stat = fs.statSync(filePath, { bigint: true })
    const record = loadWatermarkRecords()[vid]
    if (!watermarkMatches(record, filePath, Number(stat.size), stat.mtimeNs.toString(), watermarkState().worker.pipeline)) return new NextResponse(null, { status: 404 })
    const candidate = record?.candidates.find(c => c.key === key)
    if (!candidate || !/^[a-f0-9-]+\.jpg$/.test(candidate.evidence)) return new NextResponse(null, { status: 404 })
    const attempt=(record as typeof record & {artifact_attempt?:string})?.artifact_attempt
    const file = attempt?acceptedAttemptFile(attempt,path.join('watermarks/evidence',candidate.evidence)):path.join(WATERMARK_DIR,'evidence',candidate.evidence)
    if(!file)return new NextResponse(null,{status:404})
    if (!attempt&&!fs.realpathSync(file).startsWith(fs.realpathSync(path.join(WATERMARK_DIR, 'evidence')) + path.sep)) return new NextResponse(null, { status: 404 })
    return new NextResponse(fs.readFileSync(file), { headers: { 'Content-Type': 'image/jpeg', 'Cache-Control': 'private, no-store' } })
  } catch { return new NextResponse(null, { status: 404 }) }
}

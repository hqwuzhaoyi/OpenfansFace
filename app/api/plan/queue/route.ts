import { rememberPerson } from '@/lib/people-catalog'
import { healthScope } from '@/lib/video-health'
import { creatorFolder } from '@/lib/source-resolution'
import { MEDIA2 } from '@/lib/env'
import { readVideoSources, videoSource, isMediaFile } from '@/lib/video-sources'
import { NextRequest, NextResponse } from 'next/server'
import { saveQueue, saveDecisions } from '@/lib/jsonio'
import { acquireReviewLock, queuedWithOrigin, loadDecisions, loadArchive, videoInfo } from '@/lib/review-store'
import { enqueueItems, undoItems, origin, validateCreator, validateVids, ReviewError, type ReviewSource } from '@/lib/review-logic'

export async function POST(req: NextRequest) {
  let release: (() => void) | undefined
  try {
    const body = await req.json()
    const creator = validateCreator(body.creator), vids = validateVids(body.vids)
    const source: ReviewSource = ['cluster', 'person', 'approval', 'all'].includes(body.source) ? body.source : 'legacy'
    const provenance = origin(source, String(body.source_id || ''))
    const sources=readVideoSources()
    const hidden=healthScope()
    if(vids.some(v=>{const p=videoSource(v,sources);return !!p&&hidden(v,p)}))throw new ReviewError('视频体检已连续确认异常，请到回收站查看记录',409)
    if(vids.some(v=>{const p=videoSource(v,sources);return !!p&&creatorFolder(p,MEDIA2)!==null}))throw new ReviewError('creators 内的视频已归类，不再进入待归名；请刷新列表',409)
    if(vids.some(v=>{const p=videoSource(v,sources);return !p||!isMediaFile(p)}))throw new ReviewError('选中视频的源文件无法读取，请刷新列表并核对路径',409)
    release = acquireReviewLock()
    const archived = new Set(loadArchive().map(v => v.vid))
    if (vids.some(v => archived.has(v))) throw new ReviewError('部分视频已归档，请刷新列表', 409)
    const current=queuedWithOrigin(),selectedPaths=new Set<string>()
    const pendingPaths=new Map(current.map(v=>[videoSource(v.vid,sources),v.vid]))
    for(const vid of vids){const p=videoSource(vid,sources)!;if(selectedPaths.has(p)||(pendingPaths.has(p)&&pendingPaths.get(p)!==vid))throw new ReviewError('同一个源文件已在本次选择或待归档中，请刷新列表',409);selectedPaths.add(p)}
    const result = enqueueItems(current, creator, vids, provenance)
    rememberPerson(creator,videoInfo().get(vids[0])?.thumb)
    saveQueue(result.queue)
    return NextResponse.json({ ok: true, added: result.added.length, queue_len: result.queue.length, vids })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}
export async function DELETE(req: NextRequest) {
  let release: (() => void) | undefined
  try {
    const vids = validateVids((await req.json()).vids)
    release = acquireReviewLock()
    const archived = new Set(loadArchive().map(v => v.vid))
    if (vids.some(v => archived.has(v))) throw new ReviewError('部分视频已实际归档，请刷新后检查完成记录', 409)
    const result = undoItems(queuedWithOrigin(), loadDecisions(), vids)
    saveDecisions(result.decisions)
    saveQueue(result.queue)
    return NextResponse.json({ ok: true, removed: result.removed, queue_len: result.queue.length })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}

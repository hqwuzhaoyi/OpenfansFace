import { nativeArchiveEnabled, nativeArchive } from '@/lib/archive-native-backend'
import { rememberPerson } from '@/lib/people-catalog'
import { canonicalPersonName } from '@/lib/person-names'
import { isStagedPath } from '@/lib/person-file-paths'
import { healthScope } from '@/lib/video-health'
import { creatorFolder } from '@/lib/source-resolution'
import { MEDIA2 } from '@/lib/env'
import { readVideoSources, videoSource, isMediaFile } from '@/lib/video-sources'
import { NextRequest, NextResponse } from 'next/server'
import { loadClaims } from '@/lib/integration'
import { saveQueue, saveDecisions } from '@/lib/jsonio'
import { acquireReviewLock, queuedWithOrigin, loadDecisions, loadArchive, videoInfo } from '@/lib/review-store'
import { enqueueItems, origin, ReviewError, validateCreator } from '@/lib/review-logic'

export async function POST(req: NextRequest) {
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('claim-decide',await req.json()),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500})}
  let release: (() => void) | undefined
  try {
    const { vid, approve, creator } = await req.json()
    if (typeof vid !== 'string' || typeof approve !== 'boolean') throw new ReviewError('请选择视频并确认操作')
    const claim = loadClaims().find(c => c.vid === vid)
    if (!claim) throw new ReviewError('找不到这条识别推荐，请刷新', 404)
    release = acquireReviewLock()
    if (loadArchive().some(v => v.vid === vid)) throw new ReviewError('这个视频已归档，请刷新', 409)
    const decisions = loadDecisions()
    if (approve) {
      const p=videoSource(vid,readVideoSources())
      if(p&&isStagedPath(p))throw new ReviewError('暂存区中的视频不能重新归名',409)
      if(p&&healthScope()(vid,p))throw new ReviewError('视频体检已连续确认异常，请到回收站查看记录',409)
      if(p&&creatorFolder(p,MEDIA2)!==null)throw new ReviewError('creators 内的视频已归类，不需要再次审批归档；请刷新列表',409)
      if(!p||!isMediaFile(p))throw new ReviewError('原视频无法读取，请刷新列表并核对路径',409)
      const target = canonicalPersonName(validateCreator(creator ?? claim.name))
      const result = enqueueItems(queuedWithOrigin(), target, [vid], origin('approval', vid))
      rememberPerson(target,videoInfo().get(vid)?.thumb)
      saveQueue(result.queue)
      decisions.approved = [...new Set([...decisions.approved, vid])]
      decisions.rejected = decisions.rejected.filter(v => v !== vid)
    } else {
      if (queuedWithOrigin().some(v => v.vid === vid)) throw new ReviewError('视频已在待归档，请先撤销', 409)
      decisions.rejected = [...new Set([...decisions.rejected, vid])]
      decisions.approved = decisions.approved.filter(v => v !== vid)
    }
    saveDecisions(decisions)
    return NextResponse.json({ ok: true, bucket: approve ? 'approved' : 'rejected', queued: approve })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}

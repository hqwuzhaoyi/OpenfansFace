import path from 'path'
import { nativeArchive, nativeArchiveEnabled } from '@/lib/archive-native-backend'
import { NextRequest, NextResponse } from 'next/server'
import { acquireReviewLock, queuedWithOrigin } from '@/lib/review-store'
import { saveQueue } from '@/lib/jsonio'
import { buildPlan } from '@/lib/review-plan'
import { alternateName, validateArchiveName } from '@/lib/archive-files'
import { ReviewError, validateVids, type ArchiveResolution } from '@/lib/review-logic'

export async function POST(req: NextRequest) {
  let release: (() => void) | undefined
  try {
    const body = await req.json()
    if (nativeArchiveEnabled()) return NextResponse.json(await nativeArchive('archive-resolve', body))
    const vids = validateVids(body.vids), ids = new Set(vids)
    if (!['rename', 'stage-duplicate', 'clear'].includes(body.kind)) throw new ReviewError('请选择处理方式')
    release = acquireReviewLock()
    const queue = queuedWithOrigin(), plan = buildPlan(queue)
    if (body.version !== plan.version) throw new ReviewError('文件状态已变化，请刷新后重新选择', 409)
    const selected = plan.rows.filter(r => ids.has(r.vid))
    if (selected.length !== vids.length) throw new ReviewError('部分任务已不在待归档中，请刷新', 409)
    if (body.kind !== 'clear' && selected.some(r => !['duplicate', 'conflict', 'dupe-target'].includes(r.status) && !r.resolution)) throw new ReviewError('只可为同名文件选择处理方式', 409)
    if (body.kind === 'stage-duplicate' && selected.some(r => r.original_size !== r.size || r.size === 0 || !r.src)) throw new ReviewError('体积不同，无法作为重复件暂存；请选择保留两份', 409)
    const next = queue.map(item => {
      if (!ids.has(item.vid)) return item
      const row = selected.find(r => r.vid === item.vid)!
      let resolution: ArchiveResolution | undefined
      if (body.kind === 'rename') {
        let name: string
        try { name = vids.length === 1 && body.name !== undefined ? validateArchiveName(body.name) : alternateName(row.name, row.vid) }
        catch (e) { throw new ReviewError((e as Error).message) }
        if (path.extname(name).toLowerCase() !== path.extname(row.name).toLowerCase()) throw new ReviewError('请保留视频原来的扩展名')
        resolution = { kind: 'rename', name }
      } else if (body.kind === 'stage-duplicate') resolution = { kind: 'stage-duplicate' }
      return { ...item, resolution }
    })
    const prepared = buildPlan(next)
    if (body.kind !== 'clear' && prepared.rows.some(r => ids.has(r.vid) && r.status !== 'ok')) throw new ReviewError('所选路径仍存在冲突，请换一个文件名或刷新检查', 409)
    saveQueue(next)
    return NextResponse.json({ ok: true, prepared: vids.length, version: prepared.version })
  } catch (e: any) { return NextResponse.json({ error: e.message }, { status: e.status || 500 }) }
  finally { release?.() }
}

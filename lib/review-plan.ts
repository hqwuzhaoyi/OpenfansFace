import fs from 'fs'
import path from 'path'
import { createHash } from 'crypto'
import { creatorFolder } from './source-resolution'
import { MEDIA2 } from './env'
import { CREATORS } from './env'
import { readVideoSources, videoSource } from './video-sources'
import { queuedWithOrigin, videoInfo } from './review-store'
import { validateCreator, type QueueItem } from './review-logic'
import { alternateName, validateArchiveName } from './archive-files'
import { thumbUrl } from './db'
import { healthScope } from './video-health'

export interface PlanRow extends QueueItem {
  name: string; thumb: string | null; size_gb: number; src: string | null; dst: string | null
  status: string; reason: string; size: number; mtime: number
  original_dst?: string; dst_size?: number; dst_mtime?: number; suggested_name?: string
  original_size?: number
  operation: 'archive' | 'stage-duplicate'
}
export function buildPlan(queue = queuedWithOrigin()) {
  const meta = videoInfo(), targets = new Set<string>(), sources = readVideoSources()
  const hidden=healthScope()
  const rows: PlanRow[] = queue.map(item => {
    const v = meta.get(item.vid), src = videoSource(item.vid,sources)
    const row: PlanRow = { ...item, name: src ? path.basename(src) : v?.name || item.vid.replace(/\\/g, '/').split('/').pop() || item.vid,
      thumb: thumbUrl(v?.thumb), src, dst: null, size_gb: 0, size: 0, mtime: 0, status: 'ok', reason: '可以归档', operation: 'archive' }
    try { validateCreator(item.creator) } catch { return { ...row, status: 'invalid', reason: '人物名包含无效路径，请撤销后重新归名' } }
    if (src) row.dst = path.join(CREATORS, item.creator, path.basename(src))
    if (!src || !fs.existsSync(/*turbopackIgnore: true*/ src)) return { ...row, status: 'missing', reason: '记录中的原路径找不到文件；退回待确认不会找回文件，请核对实际位置或刷新状态' }
    try {
      const stat = fs.statSync(/*turbopackIgnore: true*/ src)
      if(creatorFolder(src,MEDIA2)!==null)return {...row,status:'organized',reason:'文件已在 creators 归类，无需再次归名或移动，可退回待确认'}
      if(!stat.isFile())return {...row,status:'not-file',reason:'记录指向文件夹，需核对实际视频路径'}
      if(!stat.size)return {...row,status:'empty',reason:'源文件为 0 字节，无法归档；可到回收站查看视频体检记录'}
      if(hidden(item.vid,src))return {...row,status:'corrupt',reason:'视频体检连续确认异常，已收起待确认入口；请到回收站查看原因和路径'}
      row.size = stat.size; row.size_gb = +(stat.size / 2 ** 30).toFixed(2); row.mtime = stat.mtimeMs
      row.dst = path.join(CREATORS, item.creator, path.basename(src))
      row.original_dst = row.dst
      if (fs.existsSync(row.original_dst)) {
        const original = fs.statSync(row.original_dst)
        if (original.isFile()) row.original_size = original.size
      }
      row.suggested_name = alternateName(path.basename(src), item.vid)
      if (item.resolution?.kind === 'rename') {
        try { row.dst = path.join(CREATORS, item.creator, validateArchiveName(item.resolution.name)) }
        catch { return { ...row, status: 'invalid', reason: '保留两份的文件名无效，请重新选择处理方式' } }
        row.reason = '已选择保留两份，将按新文件名归档'
      }
      if (src === row.dst) { row.status = 'already'; row.reason = '文件已在目标位置，无需再次移动' }
      else if (fs.existsSync(/*turbopackIgnore: true*/ row.dst)) {
        const dest = fs.statSync(/*turbopackIgnore: true*/ row.dst)
        row.dst_size = dest.size; row.dst_mtime = dest.mtimeMs
        row.status = dest.isFile() && dest.size === stat.size ? 'duplicate' : 'conflict'
        row.reason = row.status === 'duplicate' ? '目标存在同名同体积文件，需核对是否重复' : '目标存在同名但体积不同的文件，需先处理冲突'
        if (row.status === 'duplicate' && item.resolution?.kind === 'stage-duplicate') {
          row.operation = 'stage-duplicate'; row.status = 'ok'
          row.reason = '已选择保留目标文件；执行时完整校验内容，完全一致才把源文件移入暂存'
        }
      } else if (targets.has(row.dst)) { row.status = 'dupe-target'; row.reason = '队列里另一个视频也将使用此目标文件名' }
      if (item.resolution?.kind === 'stage-duplicate' && row.operation !== 'stage-duplicate') { row.status = 'conflict'; row.reason = '目标文件已变化，无法暂存重复件，请重新选择处理方式' }
      targets.add(row.dst)
    } catch { row.status = 'missing'; row.reason = '暂时无法读取文件，请刷新后重试' }
    return row
  })
  const version = createHash('sha256').update(JSON.stringify(rows)).digest('hex')
  return { rows, n: rows.length, n_ok: rows.filter(r=>r.status==='ok').length,
    total_gb: +(rows.filter(r=>r.status==='ok').reduce((n,r)=>n+r.size,0)/2**30).toFixed(1), version }
}

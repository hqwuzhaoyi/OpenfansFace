import fs from 'fs'
import path from 'path'
import { randomUUID } from 'crypto'
import { BASE } from './env'
import { readJson, writeJson } from './jsonio'
import { unclassifiedVideos } from './unclassified'
import { ReviewError } from './review-logic'
import { loadReport } from './integration'
import { watermarkMatches, watermarkSuggestion, type WatermarkData, type WatermarkJob, type WatermarkMapping, type WatermarkProgress, type WatermarkRecord } from './watermark-model'

export const WATERMARK_DIR = path.join(BASE, 'watermarks')
export const watermarkFile = (name: string) => path.join(WATERMARK_DIR, name)
export const loadWatermarkRecords = () => readJson<Record<string, WatermarkRecord>>(watermarkFile('results.json'), {})
export const loadWatermarkMappings = () => readJson<Record<string, WatermarkMapping>>(watermarkFile('mappings.json'), {})
export function watermarkState() {
  const heartbeat = readJson<{ at?: string; error?: string; pipeline?: string }>(watermarkFile('worker.json'), {})
  const automatic = readJson<{text_pipeline?:string}>(path.join(BASE,'processing/heartbeat.json'),{})
  const age = Date.now() - Date.parse(heartbeat.at || '')
  const worker = { online: age >= -30_000 && age < 90_000, error: heartbeat.error, pipeline: automatic.text_pipeline || heartbeat.pipeline }
  const job = readJson<WatermarkJob | null>(watermarkFile('job.json'), null)
  const stored = readJson<WatermarkProgress | null>(watermarkFile('progress.json'), null)
  const progress = job && stored?.job_id === job.id ? stored : null
  return { worker, job, progress, active: !!job && (!progress || progress.status === 'running') }
}
export function watermarkData(): WatermarkData {
  const state = watermarkState(), records = loadWatermarkRecords(), mappings = loadWatermarkMappings()
  const facePeople = new Map<string, string[]>()
  for (const person of loadReport().identities || []) for (const video of person.vids || []) {
    facePeople.set(video.vid, [...(facePeople.get(video.vid) || []), person.name])
  }
  const videos = unclassifiedVideos().videos.map(video => {
    const stat = fs.statSync(video.path, { bigint: true }), record = records[video.vid]
    const watermark = watermarkMatches(record, video.path, Number(stat.size), stat.mtimeNs.toString(), state.worker.pipeline) ? record : null
    const face_candidates = facePeople.get(video.vid) || []
    return { ...video, watermark, face_candidates, ...watermarkSuggestion(watermark?.candidates || [], mappings, face_candidates) }
  })
  return { ...state, job: state.job ? { id: state.job.id, created_at: state.job.created_at, total: state.job.videos.length, vids: state.job.videos.map(v => v.vid) } : null, videos, mappings }
}

/** Caller holds the existing review lock; concurrent tabs cannot replace an active job. */
export function requestWatermarkScan(vids: string[], force = false) {
  if (vids.length > 100) throw new ReviewError('每次最多识别 100 个视频')
  const data = watermarkData()
  if (!data.worker.online || data.worker.error) throw new ReviewError(data.worker.error || '水印识别服务未连接，请先启动后台识别服务', 503)
  if (data.active) throw new ReviewError('已有识别任务，等待完成后再提交', 409)
  const eligible = new Map(data.videos.map(v => [v.vid, v]))
  const videos = vids.map(vid => {
    const video = eligible.get(vid)
    if (!video?.path) throw new ReviewError('部分视频已归名或不可用，请刷新列表', 409)
    const stat = fs.statSync(video.path, { bigint: true })
    return { vid, path: video.path, size: Number(stat.size), mtime_ns: stat.mtimeNs.toString() }
  })
  fs.mkdirSync(WATERMARK_DIR, { recursive: true })
  const job: WatermarkJob = { id: randomUUID(), created_at: new Date().toISOString(), force, videos }
  writeJson(watermarkFile('job.json'), job)
  return { id: job.id, total: videos.length }
}

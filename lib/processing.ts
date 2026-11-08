import { ReviewError } from './review-logic'
import { nativeJsonRequest } from './native-json-transport'
import { type ProcessingTask } from './processing-model'

export interface ProcessingOverviewData {
  online: boolean
  paused: boolean
  last_discovery: any
  references: any
  events: any[]
  counts: Record<string, number>
  total: number
  page: number
  limit: number
  all_count: number
  task?: ProcessingTask | null
  rows: (Omit<ProcessingTask, 'data'> & { data: { kind?: ProcessingTask['data']['kind']; suggested?: string | null; face_skipped?: boolean } })[]
  batches: { id: string; total: number; ready: number; error: number; running: number }[]
}

export async function processingOverview(params: URLSearchParams) {
  return nativeJsonRequest<ProcessingOverviewData>('v1/metadata', {
    command: 'processing-overview',
    body: Object.fromEntries(params),
  }, {
    label: '自动分析总览读取',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

export async function processingAction(action: string, vids: string[] = [], note?: string) {
  return nativeJsonRequest<{ ok: boolean }>('v1/metadata', {
    command: 'processing-control',
    body: { action, vids, ...(note === undefined ? {} : { note }) },
  }, {
    label: '自动分析控制',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

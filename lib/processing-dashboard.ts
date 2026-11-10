import { ReviewError } from './review-logic'
import { nativeJsonRequest } from './native-json-transport'
import { type ProcessingBatch, type batchTotals } from './processing-dashboard-model'
import type { HomeJob } from './home-model'

export interface ProcessingHeartbeat {
  at?: number
  protocol?: number
  max_concurrency?: number
  concurrency?: number
  active?: number
  inference?: { backend?: string; available?: boolean; name?: string; error?: string }
}

export interface ProcessingSettingsData {
  schema_version: 1
  requested: number
  maximum: number
  effective: number
  supported: boolean
  online: boolean
  last_seen: number | null
}

export interface ProcessingDashboardData {
  schema_version: 1
  preview: boolean
  preview_id?: string
  at: number
  online: boolean
  paused: boolean
  ledger_ready: boolean
  batches: ProcessingBatch[]
  totals: ReturnType<typeof batchTotals>
  current: { running: number; waiting: number; error: number; review: number }
  jobs: HomeJob[]
  history: {
    id: string
    label: string
    status: string
    stage: string
    at: number
    finished_at: number | null
    error: string | null
    batch: string | null
    execution: any
  }[]
  history_source: 'stage_runs' | 'legacy_events'
  settings: ProcessingSettingsData
  media_issue: unknown
  discovery: any
  references: any
  inference: { backend?: string; available?: boolean; name?: string; error?: string }
  events: any[]
}

export async function processingSettings() {
  return nativeJsonRequest<ProcessingSettingsData>('v1/metadata', {
    command: 'processing-settings',
    body: {},
  }, {
    label: '自动分析设置读取',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

export async function setProcessingConcurrency(value: unknown) {
  return nativeJsonRequest<ProcessingSettingsData>('v1/metadata', {
    command: 'processing-concurrency',
    body: { concurrency: value },
  }, {
    label: '自动分析并发设置',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

export async function processingDashboard() {
  const preview = process.env.PROCESSING_PREVIEW === '1'
  return nativeJsonRequest<ProcessingDashboardData>('v1/metadata', {
    command: 'processing-dashboard',
    body: {
      preview,
      ...(preview && process.env.PROCESSING_PREVIEW_ID ? { preview_id: process.env.PROCESSING_PREVIEW_ID } : {}),
    },
  }, {
    label: '批量处理操作台读取',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

export async function retryProcessingErrors(batch?: string) {
  if (batch !== undefined && (typeof batch !== 'string' || !batch || batch.length > 128)) {
    throw new ReviewError('批次格式无效')
  }
  return nativeJsonRequest<{ schema_version: 1; retried: number }>('v1/metadata', {
    command: 'processing-retry-errors',
    body: batch === undefined ? {} : { batch },
  }, {
    label: '自动分析异常重试',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}

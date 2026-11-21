import { NextRequest, NextResponse } from 'next/server'
import { ReviewError } from '@/lib/review-logic'
import { nativeJsonRequest } from '@/lib/native-json-transport'
import type { TaskListApiResponse, ProcessingTaskRecord } from '@/lib/processing-task-model'

export const runtime = 'nodejs'

export async function GET(req: NextRequest) {
  const url = req.nextUrl
  const page = Math.max(0, parseInt(url.searchParams.get('page') || '0', 10) || 0)
  const limit = Math.min(100, Math.max(1, parseInt(url.searchParams.get('limit') || '25', 10) || 25))
  const stage = url.searchParams.get('stage') || ''
  const batch = url.searchParams.get('batch_id') || url.searchParams.get('batch') || ''
  const node_id = url.searchParams.get('node_id') || ''
  const waiting_reason = url.searchParams.get('waiting_reason') || ''
  const q = url.searchParams.get('q') || ''
  const vid = url.searchParams.get('vid') || ''

  const queryParams = new URLSearchParams()
  queryParams.set('page', String(page))
  queryParams.set('limit', String(limit))
  if (stage) queryParams.set('stage', stage)
  if (batch) {
    queryParams.set('batch', batch)
    queryParams.set('batch_id', batch)
  }
  if (node_id) queryParams.set('node_id', node_id)
  if (waiting_reason) queryParams.set('waiting_reason', waiting_reason)
  if (q) queryParams.set('q', q)
  if (vid) queryParams.set('vid', vid)

  try {
    let backendData: any
    try {
      backendData = await nativeJsonRequest<any>(
        'v1/agent/tasks?' + queryParams.toString(),
        null,
        {
          label: '处理任务列表读取',
          completion: 'required',
          error: (msg, status) => new ReviewError(msg, status),
        }
      )
    } catch (e: any) {
      if (e.status === 404) {
        backendData = await nativeJsonRequest<any>(
          'v1/metadata',
          {
            command: 'processing-overview',
            body: Object.fromEntries(queryParams.entries()),
          },
          {
            label: '处理任务总览适配读取',
            completion: 'required',
            error: (msg, status) => new ReviewError(msg, status),
          }
        )
      } else {
        throw e
      }
    }

    const rows: any[] = backendData?.tasks || backendData?.rows || []
    const normalizedTasks: ProcessingTaskRecord[] = rows.map((r: any) => ({
      vid: r.vid,
      name: r.name,
      path: r.path || r.video_path || '',
      video_path: r.video_path || r.path || null,
      size: typeof r.size === 'number' && Number.isFinite(r.size) && r.size > 0 ? r.size : null,
      duration: typeof r.duration === 'number' && Number.isFinite(r.duration) && r.duration > 0 ? r.duration : null,
      stage: r.stage || 'queued',
      retry_stage: r.retry_stage || 'text',
      text_attempts: r.text_attempts || 0,
      face_attempts: r.face_attempts || 0,
      next_retry: r.next_retry || 0,
      priority: r.priority || 0,
      is_new: r.is_new ? 1 : 0,
      batch_id: r.batch_id || '',
      created_at: r.created_at || 0,
      updated_at: r.updated_at || 0,
      error: r.error || null,
      manual: Boolean(r.manual),
      deferred: Boolean(r.deferred),
      note: r.note || '',
      assigned_node_id: r.assigned_node_id ? String(r.assigned_node_id) : (r.node_id ? String(r.node_id) : null),
      attempt_id: r.attempt_id || r.run_id || null,
      lease_id: r.lease_id || null,
      coordinator_generation: r.coordinator_generation ?? null,
      lease_expires_at: r.lease_expires_at ? Number(r.lease_expires_at) : null,
      duration_ms: r.duration_ms ?? (r.execution?.duration_ms ?? null),
      waiting_reason: r.waiting_reason || (r.note?.startsWith('waiting_') ? r.note : null),
      execution_proof: r.execution_proof || r.execution || null,
      creator: r.creator || r.data?.creator || null,
      face_skipped: r.face_skipped ?? r.data?.face_skipped ?? null,
      ocr_result: r.ocr_result || r.data?.ocr_result || null,
      error_type: r.error_type || r.data?.error_type || null,
      matched_creator: r.matched_creator || r.data?.matched_creator || r.data?.creator || null,
      ocr_text: r.ocr_text || r.data?.ocr_text || r.data?.text_snippets || null,
      skip_reason: r.skip_reason || r.data?.skip_reason || null,
      attempts_history: r.attempts_history || [],
      data: r.data,
    }))

    const counts: Record<string, number> = backendData?.counts || {}
    const totalCount = backendData?.total ?? normalizedTasks.length

    const response: TaskListApiResponse = {
      schema_version: 1,
      summary: {
        total: totalCount,
        all_count: backendData?.all_count ?? totalCount,
        running_count: (counts.text || 0) + (counts.face || 0) + (counts.detect || 0) + (counts.ocr || 0) + (counts.feature || 0),
        waiting_count: (counts.queued || 0) + (counts.retry || 0) + (counts.face_queued || 0),
        error_count: counts.error || 0,
        review_count: counts.review || 0,
        evaluated_at: Date.now() / 1000,
      },
      counts,
      filters: { stage, batch, node_id, q, waiting_reason },
      page: backendData?.page ?? page,
      limit: backendData?.limit ?? limit,
      total: totalCount,
      tasks: normalizedTasks,
      task: backendData?.task
        ? normalizedTasks.find((t) => t.vid === backendData.task.vid) || undefined
        : undefined,
      online: backendData?.online ?? true,
      paused: backendData?.paused ?? false,
      review_available: backendData?.review_available ?? (counts.review || 0),
      all_count: backendData?.all_count ?? totalCount,
      batches: backendData?.batches || [],
      events: backendData?.events || [],
      references: backendData?.references || null,
      last_discovery: backendData?.last_discovery || null,
    }

    return NextResponse.json(response, {
      headers: { 'Cache-Control': 'no-store' },
    })
  } catch (e: any) {
    return NextResponse.json(
      { schema_version: 1, error: e.message || '任务列表读取失败' },
      {
        status: e.status || 503,
        headers: { 'Cache-Control': 'no-store' },
      }
    )
  }
}

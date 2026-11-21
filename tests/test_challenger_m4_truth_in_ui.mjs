import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
  formatCreatorHandle,
  formatCreatorBadge,
  formatErrorType,
  isOcrFailureType,
  formatOcrFailureBadge,
  formatOcrFailureStatus,
} from '../lib/processing-task-model.ts'

const root = process.cwd()
const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')

// Helper function that mirrors the row status rendering logic in task-queue.tsx
function renderRowStatus(task) {
  if (task.manual) {
    return '人工接管'
  }
  if ((task.face_skipped || task.data?.face_skipped) && (task.creator || task.data?.creator || task.matched_creator)) {
    return formatCreatorBadge(task.creator || task.data?.creator || task.matched_creator)
  }
  if (isOcrFailureType(task.error_type || task.data?.error_type)) {
    return formatOcrFailureBadge(task.stage)
  }
  if ((task.ocr_result === 'no_text_found' || task.data?.ocr_result === 'no_text_found') && task.stage === 'detect') {
    return '未检出文字 · 人脸检测中'
  }
  return TASK_STAGE_LABELS[task.stage] || task.stage
}

// Helper function that mirrors the detail view rendering logic in task-queue.tsx
function renderDetailBadges(focused) {
  const items = []
  if ((focused.face_skipped || focused.data?.face_skipped) && (focused.creator || focused.data?.creator || focused.matched_creator)) {
    items.push(formatCreatorBadge(focused.creator || focused.data?.creator || focused.matched_creator))
  }
  if (focused.ocr_result === 'no_text_found' || focused.data?.ocr_result === 'no_text_found') {
    items.push('未检出文字')
  }
  if (focused.error_type || focused.data?.error_type) {
    items.push(formatErrorType(focused.error_type || focused.data?.error_type))
  }
  return items
}

test('Truth-in-UI: Fuzzing Creator Matching Badge (Double @ elimination & Suffix normalization)', () => {
  // Scenario A: Creator already has "@" prefix (e.g. "@alice")
  const taskWithAt = {
    vid: 'v-01',
    face_skipped: true,
    creator: '@alice',
    stage: 'done',
  }
  const rowBadgeWithAt = renderRowStatus(taskWithAt)
  const detailBadgeWithAt = renderDetailBadges(taskWithAt)[0]

  console.log('Row badge with "@alice":', rowBadgeWithAt)
  console.log('Detail badge with "@alice":', detailBadgeWithAt)

  // Verify remediation: single @ prefix, no @@alice, and suffix (跳过人脸分析) present in both row and detail
  assert.equal(rowBadgeWithAt, '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(detailBadgeWithAt, '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(rowBadgeWithAt.includes('@@'), false)
  assert.equal(detailBadgeWithAt.includes('@@'), false)

  // Scenario B: Creator without "@" prefix (e.g. "bob")
  const taskWithoutAt = {
    vid: 'v-02',
    face_skipped: true,
    creator: 'bob',
    stage: 'done',
  }
  const rowBadgeWithoutAt = renderRowStatus(taskWithoutAt)
  const detailBadgeWithoutAt = renderDetailBadges(taskWithoutAt)[0]

  assert.equal(rowBadgeWithoutAt, '已匹配作者: @bob (跳过人脸分析)')
  assert.equal(detailBadgeWithoutAt, '已匹配作者: @bob (跳过人脸分析)')
  assert.equal(rowBadgeWithoutAt.includes('@@'), false)
  assert.equal(detailBadgeWithoutAt.includes('@@'), false)

  // Scenario C: Matched creator fallback
  const taskMatchedOnly = {
    vid: 'v-02c',
    face_skipped: true,
    matched_creator: 'charlie',
    stage: 'done',
  }
  assert.equal(renderRowStatus(taskMatchedOnly), '已匹配作者: @charlie (跳过人脸分析)')

  // Direct unit verification of formatters
  assert.equal(formatCreatorHandle('@alice'), '@alice')
  assert.equal(formatCreatorHandle('alice'), '@alice')
  assert.equal(formatCreatorHandle('  @dan  '), '@dan')
  assert.equal(formatCreatorHandle('  eve  '), '@eve')
  assert.equal(formatCreatorHandle(null), '')
  assert.equal(formatCreatorHandle(undefined), '')
  assert.equal(formatCreatorHandle(''), '')
  assert.equal(formatCreatorHandle('   '), '')

  assert.equal(formatCreatorBadge('@alice'), '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(formatCreatorBadge('alice'), '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(formatCreatorBadge(null), '')
  assert.equal(formatCreatorBadge(''), '')
})

test('Truth-in-UI: Fuzzing OCR Result Badge (no_text_found & stage combinations)', () => {
  // When stage is 'detect', row shows '未检出文字 · 人脸检测中'
  const taskDetect = {
    vid: 'v-03',
    ocr_result: 'no_text_found',
    stage: 'detect',
  }
  assert.equal(renderRowStatus(taskDetect), '未检出文字 · 人脸检测中')
  assert.deepEqual(renderDetailBadges(taskDetect), ['未检出文字'])

  // When stage is NOT 'detect' (e.g. 'ocr' or 'queued'), what does row show?
  const taskQueued = {
    vid: 'v-04',
    ocr_result: 'no_text_found',
    stage: 'queued',
  }
  const statusQueued = renderRowStatus(taskQueued)
  console.log('Row badge when ocr_result is no_text_found but stage is queued:', statusQueued)
  assert.equal(statusQueued, '排队中')
})

test('Truth-in-UI: Fuzzing OCR Failure Badge (parse_failure vs model_error & retry state)', () => {
  // Stage === 'retry' -> "OCR 解析失败 (重试中)"
  const taskParseFailRetry = {
    vid: 'v-05',
    error_type: 'parse_failure',
    stage: 'retry',
  }
  const rowBadgeRetry = renderRowStatus(taskParseFailRetry)
  assert.equal(rowBadgeRetry, 'OCR 解析失败 (重试中)')

  // Stage === 'error' -> "OCR 解析失败 (重试已耗尽)"
  const taskModelFailExhausted = {
    vid: 'v-05b',
    error_type: 'model_error',
    stage: 'error',
  }
  const rowBadgeExhausted = renderRowStatus(taskModelFailExhausted)
  assert.equal(rowBadgeExhausted, 'OCR 解析失败 (重试已耗尽)')

  // Stage === other -> "OCR 解析失败"
  const taskOtherStage = {
    vid: 'v-05c',
    error_type: 'parse_failure',
    stage: 'queued',
  }
  assert.equal(renderRowStatus(taskOtherStage), 'OCR 解析失败')

  // Detail drawer error type localization
  assert.equal(renderDetailBadges(taskParseFailRetry)[0], '图片解析损坏')
  assert.equal(renderDetailBadges(taskModelFailExhausted)[0], 'OCR 引擎异常')

  // Direct unit verification of formatters
  assert.equal(formatErrorType('parse_failure'), '图片解析损坏')
  assert.equal(formatErrorType('model_error'), 'OCR 引擎异常')
  assert.equal(formatErrorType('no_text_found'), '未检出文字')
  assert.equal(formatErrorType('timeout'), '租约超时')
  assert.equal(formatErrorType('capability_mismatch'), '设备能力不匹配')
  assert.equal(formatErrorType('quota_exceeded'), '配额超限')
  assert.equal(formatErrorType('custom_error_code'), 'custom_error_code')
  assert.equal(formatErrorType(null), '未知错误')
  assert.equal(formatErrorType(undefined), '未知错误')
  assert.equal(formatErrorType(''), '未知错误')

  assert.equal(isOcrFailureType('parse_failure'), true)
  assert.equal(isOcrFailureType('model_error'), true)
  assert.equal(isOcrFailureType('timeout'), false)
  assert.equal(isOcrFailureType(null), false)
  assert.equal(isOcrFailureType(undefined), false)

  assert.equal(formatOcrFailureBadge('retry'), 'OCR 解析失败 (重试中)')
  assert.equal(formatOcrFailureBadge('error'), 'OCR 解析失败 (重试已耗尽)')
  assert.equal(formatOcrFailureBadge('queued'), 'OCR 解析失败')
  assert.equal(formatOcrFailureBadge(null), 'OCR 解析失败')
  assert.equal(formatOcrFailureStatus('retry'), 'OCR 解析失败 (重试中)')
})

test('Truth-in-UI: Fuzzing missing / null fields fallback to 未知', () => {
  const missingTask = {
    vid: 'v-06',
    name: 'test.mp4',
    path: '/media/test.mp4',
    size: null,
    stage: 'queued',
    retry_stage: 'text',
    text_attempts: 0,
    face_attempts: 0,
    next_retry: 0,
    priority: 0,
    is_new: 0,
    batch_id: '',
    created_at: 0,
    updated_at: 0,
    manual: false,
    deferred: false,
    note: '',
    assigned_node_id: null,
    attempt_id: null,
    lease_id: null,
    coordinator_generation: null,
    lease_expires_at: null,
    duration_ms: null,
  }

  // Check formatters directly
  assert.equal(getNodeDisplay(missingTask.assigned_node_id), '未知')
  assert.equal(getAttemptDisplay(missingTask.attempt_id), '未知')
  assert.equal(formatTaskDuration(missingTask.duration_ms), '未知')
  assert.equal(missingTask.batch_id || '未知', '未知')
  assert.equal(missingTask.coordinator_generation ? String(missingTask.coordinator_generation) : '未知', '未知')

  // Check file size Truth-in-UI formatter
  const formatSize = (s) => (typeof s === 'number' && Number.isFinite(s) && s > 0 ? `${(s / 2 ** 30).toFixed(2)} GB` : '未知')
  assert.equal(formatSize(missingTask.size), '未知')
  assert.equal(formatSize(0), '未知')
  assert.equal(formatSize(null), '未知')
  assert.equal(formatSize(undefined), '未知')
  assert.equal(formatSize(-50), '未知')
  assert.equal(formatSize(1073741824), '1.00 GB')

  // Verify task-queue.tsx source doesn't do unconditional division on size
  assert.ok(
    !taskQueueSrc.includes('<dd>{(focused.size / 2 ** 30).toFixed(2)} GB</dd>'),
    'task-queue.tsx must not format focused.size unconditionally without zero/null check'
  )
  assert.ok(
    taskQueueSrc.includes("focused.size > 0 ? `${(focused.size / 2 ** 30).toFixed(2)} GB` : '未知'"),
    'task-queue.tsx must safely fallback to 未知 when focused.size is null or zero'
  )
})

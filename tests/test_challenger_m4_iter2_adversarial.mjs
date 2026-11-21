import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { NextRequest } from 'next/server.js'
import { GET } from '../app/api/processing/tasks/route.ts'
import {
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
process.env.FILE_OPERATIONS_RUST_URL = 'http://127.0.0.1:9099'

// Helper mirroring task-queue.tsx table row status (lines 446-460)
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

// Helper mirroring task-queue.tsx detail drawer badges (lines 280-299)
function renderDetailAuthorBadge(focused) {
  if ((focused.face_skipped || focused.data?.face_skipped) && (focused.creator || focused.data?.creator || focused.matched_creator)) {
    return formatCreatorBadge(focused.creator || focused.data?.creator || focused.matched_creator)
  }
  return null
}

function renderDetailErrorType(focused) {
  if (focused.error_type || focused.data?.error_type) {
    return formatErrorType(focused.error_type || focused.data?.error_type)
  }
  return null
}

// Helper mirroring task-queue.tsx detail drawer size display (line 265)
function renderTaskQueueSize(size) {
  return typeof size === 'number' && size > 0 ? `${(size / 2 ** 30).toFixed(2)} GB` : '未知'
}

test('Defect 1 Adversarial Fuzzing: Handle normalization & Double-@ prevention', () => {
  const cases = [
    { input: '@alice', expectedHandle: '@alice', expectedBadge: '已匹配作者: @alice (跳过人脸分析)' },
    { input: 'alice', expectedHandle: '@alice', expectedBadge: '已匹配作者: @alice (跳过人脸分析)' },
    { input: '  @bob  ', expectedHandle: '@bob', expectedBadge: '已匹配作者: @bob (跳过人脸分析)' },
    { input: '  bob  ', expectedHandle: '@bob', expectedBadge: '已匹配作者: @bob (跳过人脸分析)' },
    { input: '\t@charlie\n', expectedHandle: '@charlie', expectedBadge: '已匹配作者: @charlie (跳过人脸分析)' },
    { input: '', expectedHandle: '', expectedBadge: '' },
    { input: '   ', expectedHandle: '', expectedBadge: '' },
    { input: null, expectedHandle: '', expectedBadge: '' },
    { input: undefined, expectedHandle: '', expectedBadge: '' },
  ]

  for (const c of cases) {
    const handle = formatCreatorHandle(c.input)
    const badge = formatCreatorBadge(c.input)
    assert.equal(handle, c.expectedHandle, `formatCreatorHandle("${c.input}") mismatch`)
    assert.equal(badge, c.expectedBadge, `formatCreatorBadge("${c.input}") mismatch`)
    assert.equal(badge.includes('@@'), false, `Double-@ found for input: "${c.input}"`)

    if (c.expectedBadge) {
      // Row status test
      const rowStatusDirect = renderRowStatus({ face_skipped: true, creator: c.input, stage: 'done' })
      assert.equal(rowStatusDirect, c.expectedBadge)
      assert.equal(rowStatusDirect.includes('@@'), false)

      // Detail badge test
      const detailBadgeDirect = renderDetailAuthorBadge({ face_skipped: true, creator: c.input })
      assert.equal(detailBadgeDirect, c.expectedBadge)
      assert.equal(detailBadgeDirect.includes('@@'), false)

      // Nested data fallback test
      const rowStatusNested = renderRowStatus({ data: { face_skipped: true, creator: c.input }, stage: 'done' })
      assert.equal(rowStatusNested, c.expectedBadge)

      // Matched creator fallback test
      const rowStatusMatched = renderRowStatus({ face_skipped: true, matched_creator: c.input, stage: 'done' })
      assert.equal(rowStatusMatched, c.expectedBadge)
    }
  }

  // Verify that if face_skipped is falsy, creator badge is NOT rendered
  const taskFaceNotSkipped = { face_skipped: false, creator: '@alice', stage: 'detect' }
  assert.equal(renderRowStatus(taskFaceNotSkipped), '人脸检测中')
  assert.equal(renderDetailAuthorBadge(taskFaceNotSkipped), null)
})

test('Defect 2 Adversarial Fuzzing: OCR failure badge retry state & localized error types', () => {
  const ocrFailureTypes = ['parse_failure', 'model_error']

  for (const err of ocrFailureTypes) {
    assert.equal(isOcrFailureType(err), true)

    // In retry stage -> "OCR 解析失败 (重试中)"
    const taskRetry = { error_type: err, stage: 'retry' }
    assert.equal(renderRowStatus(taskRetry), 'OCR 解析失败 (重试中)')

    // In error stage -> "OCR 解析失败 (重试已耗尽)"
    const taskError = { error_type: err, stage: 'error' }
    assert.equal(renderRowStatus(taskError), 'OCR 解析失败 (重试已耗尽)')

    // In other stage (e.g. queued, text) -> "OCR 解析失败"
    const taskQueued = { error_type: err, stage: 'queued' }
    assert.equal(renderRowStatus(taskQueued), 'OCR 解析失败')

    // Nested data property fallback
    const taskNested = { data: { error_type: err }, stage: 'retry' }
    assert.equal(renderRowStatus(taskNested), 'OCR 解析失败 (重试中)')
  }

  // Non-OCR failure types should not trigger formatOcrFailureBadge
  const nonOcrTypes = ['timeout', 'capability_mismatch', 'quota_exceeded', 'network_error']
  for (const err of nonOcrTypes) {
    assert.equal(isOcrFailureType(err), false)
    const taskOther = { error_type: err, stage: 'error' }
    assert.equal(renderRowStatus(taskOther), '异常')
  }

  // Drawer error localization checks against TASK_ERROR_LABELS
  assert.equal(renderDetailErrorType({ error_type: 'parse_failure' }), '图片解析损坏')
  assert.equal(renderDetailErrorType({ error_type: 'model_error' }), 'OCR 引擎异常')
  assert.equal(renderDetailErrorType({ error_type: 'no_text_found' }), '未检出文字')
  assert.equal(renderDetailErrorType({ error_type: 'timeout' }), '租约超时')
  assert.equal(renderDetailErrorType({ error_type: 'capability_mismatch' }), '设备能力不匹配')
  assert.equal(renderDetailErrorType({ error_type: 'quota_exceeded' }), '配额超限')
  // Fallbacks
  assert.equal(renderDetailErrorType({ error_type: 'unknown_custom_enum' }), 'unknown_custom_enum')
  assert.equal(renderDetailErrorType({}), null)
  assert.equal(renderDetailErrorType({ error_type: null }), null)
  assert.equal(renderDetailErrorType({ error_type: '' }), null)
  assert.equal(formatErrorType(null), '未知错误')
  assert.equal(formatErrorType(undefined), '未知错误')
  assert.equal(formatErrorType(''), '未知错误')
})

test('Defect 3 Adversarial Fuzzing: File size zero-coercion & Truth-in-UI', async (t) => {
  // Test UI rendering logic directly on null, undefined, 0, negative, NaN
  const directSizes = [
    { input: null, expected: '未知' },
    { input: undefined, expected: '未知' },
    { input: 0, expected: '未知' },
    { input: -1, expected: '未知' },
    { input: -999999, expected: '未知' },
    { input: NaN, expected: '未知' },
    { input: '1073741824', expected: '未知' }, // non-number string
    { input: {}, expected: '未知' },
    { input: 1073741824, expected: '1.00 GB' },
    { input: 2147483648, expected: '2.00 GB' },
    { input: 536870912, expected: '0.50 GB' },
  ]

  for (const c of directSizes) {
    const rendered = renderTaskQueueSize(c.input)
    assert.equal(rendered, c.expected, `renderTaskQueueSize(${JSON.stringify(c.input)}) rendered "${rendered}" instead of "${c.expected}"`)
    assert.notEqual(rendered, '0.00 GB', `renderTaskQueueSize must NEVER render 0.00 GB for invalid/zero/missing sizes`)
  }

  // Test API route handling: ensuring backend size normalization never coerces null/0 to 0
  t.mock.method(globalThis, 'fetch', async () => {
    return Response.json({
      tasks: [
        { vid: 'v-null', name: 'null_size.mp4', size: null },
        { vid: 'v-zero', name: 'zero_size.mp4', size: 0 },
        { vid: 'v-missing', name: 'missing_size.mp4' },
        { vid: 'v-negative', name: 'negative_size.mp4', size: -100 },
        { vid: 'v-infinity', name: 'infinity_size.mp4', size: Infinity },
        { vid: 'v-nan', name: 'nan_size.mp4', size: NaN },
        { vid: 'v-valid', name: 'valid_size.mp4', size: 1073741824 },
      ],
      counts: {},
      total: 7,
    })
  })

  const req = new NextRequest('http://localhost:3000/api/processing/tasks?page=0')
  const res = await GET(req)
  assert.equal(res.status, 200)
  const data = await res.json()

  const map = new Map(data.tasks.map(t => [t.vid, t.size]))
  assert.equal(map.get('v-null'), null, 'null size must normalize to null')
  assert.equal(map.get('v-zero'), null, 'zero size must normalize to null, never coerced to 0')
  assert.equal(map.get('v-missing'), null, 'missing size must normalize to null, never coerced to 0')
  assert.equal(map.get('v-negative'), null, 'negative size must normalize to null, never coerced to 0')
  assert.equal(map.get('v-infinity'), null, 'infinity size must normalize to null')
  assert.equal(map.get('v-nan'), null, 'NaN size must normalize to null')
  assert.equal(map.get('v-valid'), 1073741824, 'valid size must be preserved')

  // Verify end-to-end rendering of the normalized tasks from API response
  for (const task of data.tasks) {
    const rendered = renderTaskQueueSize(task.size)
    if (task.vid === 'v-valid') {
      assert.equal(rendered, '1.00 GB')
    } else {
      assert.equal(rendered, '未知')
      assert.notEqual(rendered, '0.00 GB')
    }
  }
})

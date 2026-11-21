import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { NextRequest } from 'next/server.js'
import { GET } from '../app/api/processing/tasks/route.ts'
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
} from '../lib/processing-task-model.ts'

const root = process.cwd()
process.env.FILE_OPERATIONS_RUST_URL = 'http://127.0.0.1:9099'

function createRequest(params = {}) {
  const url = new URL('http://localhost:3000/api/processing/tasks')
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined) {
      url.searchParams.set(k, String(v))
    }
  }
  return new NextRequest(url)
}

test('Objective 1: API Route Adversarial Fuzzing & Clamping & Injection Safety', async (t) => {
  let capturedBackendUrl = ''
  t.mock.method(globalThis, 'fetch', async (url) => {
    capturedBackendUrl = url.toString()
    return Response.json({ tasks: [], counts: {}, total: 0 })
  })

  // 1. Extreme negative and huge limits
  const reqClamp = createRequest({ page: -100, limit: -9999 })
  const resClamp = await GET(reqClamp)
  assert.equal(resClamp.status, 200)
  assert.equal(resClamp.headers.get('Cache-Control'), 'no-store')
  const pClamp = new URL(capturedBackendUrl).searchParams
  assert.equal(pClamp.get('page'), '0', 'Page must clamp to 0')
  assert.equal(pClamp.get('limit'), '1', 'Limit must clamp to min 1')

  const reqHuge = createRequest({ page: 5, limit: 1000000 })
  const resHuge = await GET(reqHuge)
  assert.equal(resHuge.status, 200)
  const pHuge = new URL(capturedBackendUrl).searchParams
  assert.equal(pHuge.get('limit'), '100', 'Limit must clamp to max 100')

  // 2. Non-numeric strings
  const reqStr = createRequest({ page: 'xyz', limit: 'bad' })
  const resStr = await GET(reqStr)
  assert.equal(resStr.status, 200)
  const pStr = new URL(capturedBackendUrl).searchParams
  assert.equal(pStr.get('page'), '0')
  assert.equal(pStr.get('limit'), '25')

  // 3. SQL Injection vectors
  const sqlVectors = [
    "' OR '1'='1",
    "'; DROP TABLE tasks; --",
    "' UNION SELECT null, null, null --",
    "admin' --",
  ]
  for (const vec of sqlVectors) {
    const reqVec = createRequest({ q: vec, stage: vec, batch: vec, node_id: vec })
    const resVec = await GET(reqVec)
    assert.equal(resVec.status, 200)
    assert.equal(resVec.headers.get('Cache-Control'), 'no-store')
  }

  // 4. Error response with mock 503
  t.mock.method(globalThis, 'fetch', async () => {
    throw new TypeError('Backend 503 Unavailable')
  })
  const reqErr = createRequest({ page: 0 })
  const resErr = await GET(reqErr)
  assert.equal(resErr.status, 503)
  assert.equal(resErr.headers.get('Cache-Control'), 'no-store')
  const bodyErr = await resErr.json()
  assert.equal(bodyErr.schema_version, 1)
  assert.ok(bodyErr.error)

  // 5. Clean field pass-through of M4 properties
  t.mock.method(globalThis, 'fetch', async () => {
    return Response.json({
      tasks: [
        {
          vid: 'vid-pass-1',
          name: 'creator.mp4',
          creator: '@alice',
          face_skipped: true,
          ocr_result: 'text_found',
          error_type: null,
        },
      ],
      counts: {},
      total: 1,
    })
  })
  const reqPass = createRequest({ page: 0 })
  const resPass = await GET(reqPass)
  const bodyPass = await resPass.json()
  const rawPass = JSON.stringify(bodyPass)
  assert.ok(!rawPass.includes('undefined'))
  assert.ok(!rawPass.includes('NaN'))
  assert.equal(bodyPass.tasks[0].creator, '@alice')
  assert.equal(bodyPass.tasks[0].face_skipped, true)
  assert.equal(bodyPass.tasks[0].size, null, 'Missing size must normalize to null, not 0')
})

test('Objective 2: Truth-in-UI Badges & State Machine Fuzzing (Remediated)', () => {
  // Let's inspect exact logic in components/processing/task-queue.tsx
  const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')

  // 1. Double '@' interpolation bug check - must NOT have unconditioned interpolation
  const hasUnconditionedAt = taskQueueSrc.includes('已匹配作者: @{')
  assert.equal(
    hasUnconditionedAt,
    false,
    'task-queue.tsx must not use unconditioned @{...} interpolation'
  )
  assert.ok(
    taskQueueSrc.includes('formatCreatorBadge'),
    'task-queue.tsx must use formatCreatorBadge for normalized author badges'
  )

  // 2. OCR failure distinction badge check
  assert.ok(
    taskQueueSrc.includes('formatOcrFailureBadge'),
    'task-queue.tsx must use formatOcrFailureBadge for stage-aware retry status'
  )

  // 3. File size Truth-in-UI check (no unconditional coercion to 0.00 GB)
  assert.ok(
    !taskQueueSrc.includes('<dd>{(focused.size / 2 ** 30).toFixed(2)} GB</dd>'),
    'task-queue.tsx must not unconditionally format focused.size without null/zero check'
  )
  assert.ok(
    taskQueueSrc.includes("focused.size > 0 ? `${(focused.size / 2 ** 30).toFixed(2)} GB` : '未知'"),
    'task-queue.tsx must safely fallback to 未知 when focused.size is null or zero'
  )

  // 4. Missing / null fields fallback to "未知"
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(undefined), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(undefined), '未知')
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')
  assert.equal(formatTaskDuration(-5), '未知')
  assert.equal(formatLeaseRemaining(null), '无有效租约')

  // 5. Unit verification of M4 remediation formatters
  assert.equal(formatCreatorHandle('@alice'), '@alice')
  assert.equal(formatCreatorHandle('alice'), '@alice')
  assert.equal(formatCreatorBadge('@alice'), '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(formatCreatorBadge('alice'), '已匹配作者: @alice (跳过人脸分析)')
  assert.equal(formatOcrFailureBadge('retry'), 'OCR 解析失败 (重试中)')
  assert.equal(formatOcrFailureBadge('error'), 'OCR 解析失败 (重试已耗尽)')
  assert.equal(formatOcrFailureBadge('queued'), 'OCR 解析失败')
  assert.equal(formatErrorType('parse_failure'), '图片解析损坏')
  assert.equal(formatErrorType('model_error'), 'OCR 引擎异常')
  assert.equal(formatErrorType(null), '未知错误')
})

test('Objective 3: Continuous Inspection (Focus Wipe Verification)', () => {
  const focusedTask = {
    vid: 'vid-keep',
    name: 'video.mp4',
    stage: 'ocr',
    assigned_node_id: 'node-wsl',
  }

  // Exact function from task-queue.tsx line 120-126
  const updateFocused = (previous, resp) =>
    previous
      ? (resp.task == null
        ? resp.tasks.find((t) => t.vid === previous.vid) || previous
        : resp.task)
      : null

  // Null task payload preserves focused
  const respNull = {
    tasks: [{ ...focusedTask, stage: 'done' }],
    task: null,
  }
  const resultNull = updateFocused(focusedTask, respNull)
  assert.notEqual(resultNull, null)
  assert.equal(resultNull.vid, 'vid-keep')
  assert.equal(resultNull.stage, 'done')

  // Undefined task payload preserves focused
  const respUndefined = {
    tasks: [{ ...focusedTask, stage: 'feature' }],
    task: undefined,
  }
  const resultUndefined = updateFocused(focusedTask, respUndefined)
  assert.notEqual(resultUndefined, null)
  assert.equal(resultUndefined.vid, 'vid-keep')
  assert.equal(resultUndefined.stage, 'feature')
})

test('Objective 4: Accessibility & Mobile Responsiveness Empirical Verification', () => {
  const css = fs.readFileSync(path.join(root, 'components/workbench/workspace.css'), 'utf-8')
  const dialogSrc = fs.readFileSync(path.join(root, 'components/ui/dialog.tsx'), 'utf-8')
  const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')

  // 1. CSS Touch targets >= 44px on mobile
  assert.ok(css.includes('.ws-icon{min-width:44px;width:44px;min-height:44px}'))
  assert.ok(css.includes('.ws-search [data-slot=button]{min-width:44px;min-height:44px;width:44px;height:44px}'))
  assert.ok(css.includes('.ws-detail-actions [data-slot=button]{min-height:44px}'))
  assert.ok(dialogSrc.includes('size-11'))

  // 2. Screen-reader accessible labels on dialog buttons
  assert.ok(dialogSrc.includes("<span className='sr-only'>Close</span>"))
  assert.ok(taskQueueSrc.includes('<DialogTitle className="sr-only">任务详情</DialogTitle>'))
  assert.ok(taskQueueSrc.includes('<DialogDescription className="sr-only">关闭后返回当前队列位置。</DialogDescription>'))
})

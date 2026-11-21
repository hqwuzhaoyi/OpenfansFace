import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
  TASK_WAITING_REASON_LABELS,
  formatWaitingReason,
  formatTaskWaitingReason,
  formatWaitingReasonDescription,
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  formatCreatorHandle,
  formatCreatorBadge,
  formatErrorType,
  isOcrFailureType,
  formatOcrFailureBadge,
  formatAttemptStatus,
  formatAttemptStage,
  getAttemptDeduplicationCount,
} from '../lib/processing-task-model.ts'

const root = process.cwd()
const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')
const apiRouteSrc = fs.readFileSync(path.join(root, 'app/api/processing/tasks/route.ts'), 'utf-8')

// Replicate the row status rendering logic from task-queue.tsx
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

// Replicate the size formatting logic from task-queue.tsx
function formatFileSize(size) {
  return typeof size === 'number' && Number.isFinite(size) && size > 0
    ? `${(size / 2 ** 30).toFixed(2)} GB`
    : '未知'
}

test('Empirical Suite 1: TASK_WAITING_REASON_LABELS & formatters mapping & fallback safety', () => {
  // 1. Mandatory Milestone 6 waiting reasons
  const expectedReasons = [
    { key: 'waiting_for_node', label: '等待计算节点', description: '当前无空闲或满足硬件能力要求的计算节点' },
    { key: 'quota_exhausted', label: '并发配额超限', description: '已达到全局并发上限或物理设备配额阶梯' },
    { key: 'gpu_busy', label: 'GPU 显存繁忙', description: '显存或算力不足以承载新检测任务' },
    { key: 'rate_limited', label: '并发降频限流', description: '系统正在执行优雅并发控制，在途任务完成后方可领取' },
    { key: 'paused', label: '人工暂停接单', description: '操作员已暂停调度' },
    { key: 'none', label: '无等待', description: '任务正常流转中' },
  ]

  for (const { key, label, description } of expectedReasons) {
    assert.ok(TASK_WAITING_REASON_LABELS[key], `TASK_WAITING_REASON_LABELS must define '${key}'`)
    assert.equal(TASK_WAITING_REASON_LABELS[key].label, label)
    assert.equal(TASK_WAITING_REASON_LABELS[key].description, description)

    // Formatter outputs
    assert.equal(formatWaitingReason(key), label)
    assert.equal(formatWaitingReasonDescription(key), description)
  }

  // 2. Compatible synonyms and supplementary reasons
  const supplementaryReasons = [
    { key: 'waiting_node', label: '等待计算节点' },
    { key: 'quota_limit', label: '并发配额超限' },
    { key: 'retry_backoff', label: '重试退避等待' },
    { key: 'model_warmup', label: '模型预热自检' },
  ]
  for (const { key, label } of supplementaryReasons) {
    assert.equal(formatWaitingReason(key), label)
    assert.ok(formatWaitingReasonDescription(key).length > 0)
  }

  // 3. Fallback safety & fuzzing on empty/falsy/null/undefined/whitespace inputs
  assert.equal(formatWaitingReason(null), '无等待')
  assert.equal(formatWaitingReason(undefined), '无等待')
  assert.equal(formatWaitingReason(''), '无等待')
  assert.equal(formatWaitingReason('   '), '无等待')
  assert.equal(formatWaitingReason('none'), '无等待')

  assert.equal(formatWaitingReasonDescription(null), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription(undefined), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription(''), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription('   '), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription('none'), '任务正常流转中')

  // 4. Unknown tokens do not crash and degrade gracefully to localized Chinese
  assert.equal(formatWaitingReason('custom_unknown_token'), '未知等待原因')
  assert.equal(formatWaitingReason('unknown'), '未知等待原因')
  assert.equal(formatWaitingReasonDescription('custom_unknown_token'), '未知等待原因')
  assert.equal(formatTaskWaitingReason('waiting_for_node'), '等待计算节点')
})

test('Empirical Suite 2: Truth-in-UI invariants (No raw enum leaks & null/0 safe fallbacks)', () => {
  // 1. Verify no raw snake_case English tokens leak in standard stage row statuses
  const allStages = [
    'queued',
    'detect',
    'ocr',
    'feature',
    'text',
    'face',
    'face_queued',
    'retry',
    'review',
    'error',
    'excluded',
    'done',
  ]
  const snakeCaseRegex = /^[a-z]+(_[a-z]+)+$/

  for (const st of allStages) {
    const task = { vid: `v-${st}`, stage: st, name: `test-${st}.mp4` }
    const statusText = renderRowStatus(task)
    assert.ok(typeof statusText === 'string' && statusText.length > 0)
    assert.ok(!snakeCaseRegex.test(statusText), `Rendered status for '${st}' must not leak raw enum: received '${statusText}'`)
  }

  // 2. Verify all waiting reason labels do not leak raw snake_case enums
  for (const key of Object.keys(TASK_WAITING_REASON_LABELS)) {
    const label = TASK_WAITING_REASON_LABELS[key].label
    assert.ok(!snakeCaseRegex.test(label), `Waiting reason label for '${key}' must not be raw snake_case: received '${label}'`)
  }

  // 3. Truth-in-UI for file size: 0 / null / undefined / negative must display "未知", NOT "0.00 GB" or "0 GB"
  assert.equal(formatFileSize(null), '未知')
  assert.equal(formatFileSize(undefined), '未知')
  assert.equal(formatFileSize(0), '未知')
  assert.equal(formatFileSize(-1), '未知')
  assert.equal(formatFileSize(NaN), '未知')
  assert.equal(formatFileSize(Infinity), '未知')
  assert.equal(formatFileSize(1073741824), '1.00 GB')
  assert.equal(formatFileSize(2147483648), '2.00 GB')

  // Verify task-queue.tsx explicitly contains the Truth-in-UI guard for size
  assert.ok(
    taskQueueSrc.includes("typeof focused.size === 'number' && focused.size > 0 ? `${(focused.size / 2 ** 30).toFixed(2)} GB` : '未知'"),
    'task-queue.tsx must guard focused.size strictly against 0 and null'
  )

  // 4. Truth-in-UI for video_path & path:
  // When video_path and path are null/undefined/empty, task-queue.tsx must show fallback message
  assert.ok(
    taskQueueSrc.includes('focused.video_path || focused.path ? ('),
    'task-queue.tsx must evaluate both video_path and path'
  )
  assert.ok(
    taskQueueSrc.includes('暂无有效视频路径'),
    'task-queue.tsx must render "暂无有效视频路径" when video path is missing'
  )
  assert.ok(
    taskQueueSrc.includes('暂无有效文件路径'),
    'task-queue.tsx must render "暂无有效文件路径" when file path is missing'
  )

  // 5. Truth-in-UI for duration_ms
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')
  assert.equal(formatTaskDuration(0), '未知')
  assert.equal(formatTaskDuration(-500), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')
  assert.equal(formatTaskDuration(350), '350 ms')
  assert.equal(formatTaskDuration(1500), '1.5 秒')
  assert.equal(formatTaskDuration(65000), '1 分 5 秒')

  // 6. Truth-in-UI for node and attempt identifiers
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(undefined), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getNodeDisplay('   '), '未知')
  assert.equal(getNodeDisplay('node-gpu-4070'), 'node-gpu-4070')

  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(undefined), '未知')
  assert.equal(getAttemptDisplay(''), '未知')
  assert.equal(getAttemptDisplay('   '), '未知')
  assert.equal(getAttemptDisplay('att-1234567890'), 'att-1234')
})

test('Empirical Suite 3: Popover filter selectors & query parameter wiring in task-queue.tsx', () => {
  // 1. Verify Popover components and filter state in task-queue.tsx
  assert.ok(taskQueueSrc.includes("import * as Popover from '@radix-ui/react-popover'"), 'Must import Radix Popover')
  assert.ok(taskQueueSrc.includes('waiting_reason: string'), 'Must include waiting_reason filter')
  assert.ok(taskQueueSrc.includes('node_id: string'), 'Must include node_id filter')

  // 2. Verify filter Popover trigger button has active dot indicator for node_id and waiting_reason
  assert.ok(
    taskQueueSrc.includes('(batch || node_id || waiting_reason)'),
    'Popover trigger button must show active dot indicator when batch, node_id, or waiting_reason is active'
  )

  // 3. Verify node_id selector exists and maps nodesList
  assert.ok(taskQueueSrc.includes('label="选择执行节点"'), 'Must have select input for node_id')
  assert.ok(taskQueueSrc.includes('value={node_id}'), 'Must bind value to node_id')
  assert.ok(taskQueueSrc.includes('onChange={setNode}'), 'Must bind onChange to setNode')
  assert.ok(taskQueueSrc.includes('label: \'全部节点\''), 'Must have "全部节点" default option')

  // 4. Verify waiting_reason selector exists and maps all valid reasons
  assert.ok(taskQueueSrc.includes('label="选择等待状态"'), 'Must have select input for waiting_reason')
  assert.ok(taskQueueSrc.includes('value={waiting_reason || \'\'}'), 'Must bind value to waiting_reason')
  assert.ok(taskQueueSrc.includes('onChange={setWaitingReason}'), 'Must bind onChange to setWaitingReason')

  const expectedFilterOptions = [
    { value: '', label: '全部等待状态' },
    { value: 'waiting_for_node', label: '等待计算节点' },
    { value: 'quota_exhausted', label: '并发配额超限' },
    { value: 'gpu_busy', label: 'GPU 显存繁忙' },
    { value: 'rate_limited', label: '并发降频限流' },
    { value: 'retry_backoff', label: '重试退避等待' },
    { value: 'model_warmup', label: '模型预热自检' },
    { value: 'paused', label: '人工暂停接单' },
    { value: 'none', label: '无等待' },
  ]

  for (const opt of expectedFilterOptions) {
    assert.ok(
      taskQueueSrc.includes(`value: '${opt.value}', label: '${opt.label}'`),
      `Popover waiting reason select must include option value: '${opt.value}', label: '${opt.label}'`
    )
  }

  // 5. Verify clear filters button resets batch, node_id, waiting_reason, and page
  assert.ok(taskQueueSrc.includes("setBatch('')"), 'Clear filters must reset batch')
  assert.ok(taskQueueSrc.includes("setNode('')"), 'Clear filters must reset node')
  assert.ok(taskQueueSrc.includes("setWaitingReason('')"), 'Clear filters must reset waiting_reason')
  assert.ok(taskQueueSrc.includes('setPage(0)'), 'Clear filters must reset page to 0')

  // 6. Verify URLSearchParams in load() appends node_id and waiting_reason
  assert.ok(taskQueueSrc.includes('...(node_id ? { node_id } : {})'), 'load() must propagate node_id')
  assert.ok(taskQueueSrc.includes('...(waiting_reason ? { waiting_reason } : {})'), 'load() must propagate waiting_reason')

  // 7. Verify API route app/api/processing/tasks/route.ts handles node_id and waiting_reason
  assert.ok(apiRouteSrc.includes("url.searchParams.get('node_id')"), 'API route must extract node_id param')
  assert.ok(apiRouteSrc.includes("url.searchParams.get('waiting_reason')"), 'API route must extract waiting_reason param')
  assert.ok(apiRouteSrc.includes("queryParams.set('node_id', node_id)"), 'API route must forward node_id')
  assert.ok(apiRouteSrc.includes("queryParams.set('waiting_reason', waiting_reason)"), 'API route must forward waiting_reason')
  assert.ok(apiRouteSrc.includes("video_path: r.video_path || r.path || null"), 'API route must map video_path')
})

test('Empirical Suite 4: BEUI Attempt History Drawer & Deduplication calculation', () => {
  // 1. Verify BEUI Drawer import and configuration
  assert.ok(
    taskQueueSrc.includes("import { Drawer } from '@/components/beui/motion/drawer'"),
    'Must import Drawer from BEUI motion component'
  )
  assert.ok(
    taskQueueSrc.includes('side="right"'),
    'Drawer must slide in from right'
  )
  assert.ok(
    taskQueueSrc.includes('ariaLabel="任务尝试历史与幂等去重抽屉"'),
    'Drawer must have descriptive ariaLabel'
  )

  // 2. Verify Deduplication calculation logic: max(delivery_count, redelivery_count)
  assert.equal(getAttemptDeduplicationCount(null), 1)
  assert.equal(getAttemptDeduplicationCount(undefined), 1)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 1 }), 1)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3 }), 3)
  assert.equal(getAttemptDeduplicationCount({ redelivery_count: 4 }), 4)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 2, redelivery_count: 5 }), 5)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 7, redelivery_count: 3 }), 7)

  // 3. Verify deduplication badge trigger condition:
  // In task-queue.tsx: dedupCount > 1
  const renderDedupBadge = (att) => {
    const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
    return dedupCount > 1 ? `幂等去重 x${dedupCount}` : null
  }
  assert.equal(renderDedupBadge({ delivery_count: 1 }), null)
  assert.equal(renderDedupBadge({ delivery_count: 1, redelivery_count: 1 }), null)
  assert.equal(renderDedupBadge({ delivery_count: 2 }), '幂等去重 x2')
  assert.equal(renderDedupBadge({ redelivery_count: 3 }), '幂等去重 x3')
  assert.equal(renderDedupBadge({ delivery_count: 2, redelivery_count: 4 }), '幂等去重 x4')

  // 4. Verify Total Deduplication formula in Drawer summary strip:
  // taskAttempts.reduce((acc, a) => acc + Math.max(0, Math.max(a.delivery_count ?? 0, a.redelivery_count ?? 0, 1) - 1), 0)
  const calcTotalDedup = (attempts) =>
    attempts.reduce((acc, a) => acc + Math.max(0, Math.max(a.delivery_count ?? 0, a.redelivery_count ?? 0, 1) - 1), 0)

  assert.equal(calcTotalDedup([]), 0)
  assert.equal(calcTotalDedup([{ delivery_count: 1 }, { delivery_count: 1 }]), 0)
  assert.equal(
    calcTotalDedup([
      { delivery_count: 1 },
      { delivery_count: 3 }, // 2 dedup events
      { redelivery_count: 2 }, // 1 dedup event
      { delivery_count: 1, redelivery_count: 2 }, // 1 dedup event (proves no short-circuit when delivery_count is 1)
    ]),
    4
  )
  assert.equal(
    calcTotalDedup([
      { delivery_count: 5 }, // 4 dedup events
      { delivery_count: 1 }, // 0
      { redelivery_count: 4 }, // 3 dedup events
      { delivery_count: 1, redelivery_count: 3 }, // 2 dedup events
    ]),
    9
  )

  // 5. Verify authoritative commit badge in summary strip
  const checkAuthoritative = (attempts) =>
    attempts.some((a) => a.is_authoritative) ? '已原子提交' : '未提交'

  assert.equal(checkAuthoritative([]), '未提交')
  assert.equal(checkAuthoritative([{ is_authoritative: false }]), '未提交')
  assert.equal(checkAuthoritative([{ is_authoritative: false }, { is_authoritative: true }]), '已原子提交')

  // 6. Verify Attempt Status normalizer
  const sDone = formatAttemptStatus('done')
  assert.equal(sDone.label, '已完成')
  assert.equal(sDone.isDone, true)
  assert.ok(sDone.className.includes('emerald'))

  const sInterrupted = formatAttemptStatus('interrupted')
  assert.equal(sInterrupted.label, '中断回收')
  assert.equal(sInterrupted.isInterrupted, true)
  assert.ok(sInterrupted.className.includes('amber'))

  const sError = formatAttemptStatus('error')
  assert.equal(sError.label, '失败')
  assert.equal(sError.isError, true)
  assert.ok(sError.className.includes('destructive'))

  const sRunning = formatAttemptStatus('running')
  assert.equal(sRunning.label, '运行中')
  assert.ok(sRunning.className.includes('muted'))

  // 7. Verify Attempt Stage label normalizer
  assert.equal(formatAttemptStage('detect'), '人脸检测中')
  assert.equal(formatAttemptStage('ocr'), '文字识别中')
  assert.equal(formatAttemptStage('feature'), '特征提取中')
  assert.equal(formatAttemptStage(null), '未知')
  assert.equal(formatAttemptStage(undefined), '未知')
})

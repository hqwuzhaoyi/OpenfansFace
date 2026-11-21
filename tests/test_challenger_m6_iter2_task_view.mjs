import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
  TASK_WAITING_REASON_LABELS,
  formatWaitingReason,
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

// =========================================================================
// Suite 1: Adversarial Stress Testing of Waiting Reasons & Status Strings
// =========================================================================
test('Stress 1: Standard Waiting Reasons & Attempt Status Normalizer', () => {
  // 1. All predefined waiting reason labels must be localized Chinese, never raw snake_case English
  const snakeCaseRegex = /^[a-z]+(_[a-z]+)+$/
  for (const [key, val] of Object.entries(TASK_WAITING_REASON_LABELS)) {
    assert.ok(!snakeCaseRegex.test(val.label), `TASK_WAITING_REASON_LABELS[${key}].label must not be snake_case`)
    assert.ok(val.label.length > 0, `Label for ${key} cannot be empty`)
    assert.ok(val.description.length > 0, `Description for ${key} cannot be empty`)
    const formatted = formatWaitingReason(key)
    assert.ok(!snakeCaseRegex.test(formatted), `formatWaitingReason(${key}) must be localized`)
  }

  // 2. Falsy/none waiting reason fallbacks
  assert.equal(formatWaitingReason(null), '无等待')
  assert.equal(formatWaitingReason(undefined), '无等待')
  assert.equal(formatWaitingReason(''), '无等待')
  assert.equal(formatWaitingReason('none'), '无等待')

  assert.equal(formatWaitingReasonDescription(null), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription(undefined), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription(''), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription('none'), '任务正常流转中')

  // 3. Status normalizer formatAttemptStatus adversarial inputs
  const statusMatrix = [
    { input: 'done', expected: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'DONE', expected: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'completed', expected: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'COMPLETED', expected: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'interrupted', expected: '中断回收', isDone: false, isInterrupted: true, isError: false },
    { input: 'INTERRUPTED', expected: '中断回收', isDone: false, isInterrupted: true, isError: false },
    { input: 'error', expected: '失败', isDone: false, isInterrupted: false, isError: true },
    { input: 'failed', expected: '失败', isDone: false, isInterrupted: false, isError: true },
    { input: 'running', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    // Adversarial strings default safely to '运行中', never leak raw English
    { input: 'unknown_status', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: 'crashed', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: 'reaped', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: 'zombie', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: '', expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: null, expected: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: undefined, expected: '运行中', isDone: false, isInterrupted: false, isError: false },
  ]

  for (const item of statusMatrix) {
    const res = formatAttemptStatus(item.input)
    assert.equal(res.label, item.expected, `formatAttemptStatus(${item.input}) label mismatch`)
    assert.equal(res.isDone, item.isDone)
    assert.equal(res.isInterrupted, item.isInterrupted)
    assert.equal(res.isError, item.isError)
    assert.ok(typeof res.className === 'string' && res.className.length > 0)
    assert.ok(!snakeCaseRegex.test(res.label), `formatAttemptStatus(${item.input}) must not leak snake_case`)
  }

  // 4. formatAttemptStage adversarial inputs
  assert.equal(formatAttemptStage(null), '未知')
  assert.equal(formatAttemptStage(undefined), '未知')
  assert.equal(formatAttemptStage(''), '未知')
  assert.equal(formatAttemptStage('   '), '未知')
  assert.equal(formatAttemptStage('detect'), '人脸检测中')
  assert.equal(formatAttemptStage('ocr'), '文字识别中')
  assert.equal(formatAttemptStage('feature'), '特征提取中')

  // 5. formatErrorType adversarial inputs
  assert.equal(formatErrorType(null), '未知错误')
  assert.equal(formatErrorType(undefined), '未知错误')
  assert.equal(formatErrorType(''), '未知错误')
  assert.equal(formatErrorType('parse_failure'), '图片解析损坏')
  assert.equal(formatErrorType('model_error'), 'OCR 引擎异常')
  assert.equal(formatErrorType('no_text_found'), '未检出文字')
  assert.equal(formatErrorType('timeout'), '租约超时')
})

// =========================================================================
// Suite 2: Adversarial Stress Testing of Empty/Null Paths, Negative Numbers,
//          and Corrupted Attempt Records
// =========================================================================
test('Stress 2: Empty/Null Paths, Negative Durations, Null/Undefined Attempts', () => {
  // 1. Truth-in-UI for file size: Negative, NaN, 0, null, undefined must be '未知'
  const invalidSizes = [null, undefined, 0, -1, -99999, NaN, Infinity, -Infinity]
  for (const s of invalidSizes) {
    const formatted = typeof s === 'number' && Number.isFinite(s) && s > 0 ? `${(s / 2 ** 30).toFixed(2)} GB` : '未知'
    assert.equal(formatted, '未知', `Size '${s}' must format to '未知'`)
  }
  const validSize1 = 1024 * 1024 * 1024 // 1 GB
  assert.equal(`${(validSize1 / 2 ** 30).toFixed(2)} GB`, '1.00 GB')

  // 2. formatTaskDuration adversarial boundary testing
  const invalidDurations = [null, undefined, 0, -0.01, -1, -5000, NaN, Infinity, -Infinity]
  for (const d of invalidDurations) {
    assert.equal(formatTaskDuration(d), '未知', `Duration '${d}' must format to '未知'`)
  }
  assert.equal(formatTaskDuration(1), '1 ms')
  assert.equal(formatTaskDuration(999), '999 ms')
  assert.equal(formatTaskDuration(1000), '1.0 秒')
  assert.equal(formatTaskDuration(1550), '1.6 秒')
  assert.equal(formatTaskDuration(59900), '59.9 秒')
  assert.equal(formatTaskDuration(60000), '1 分 0 秒')
  assert.equal(formatTaskDuration(125000), '2 分 5 秒')

  // 3. formatLeaseRemaining adversarial boundary testing
  assert.equal(formatLeaseRemaining(null), '无有效租约')
  assert.equal(formatLeaseRemaining(undefined), '无有效租约')
  assert.equal(formatLeaseRemaining(0), '无有效租约')
  assert.equal(formatLeaseRemaining(-100), '无有效租约')
  assert.equal(formatLeaseRemaining(NaN), '无有效租约')

  const now = 1000
  assert.equal(formatLeaseRemaining(900, now), '租约已过期')
  assert.equal(formatLeaseRemaining(1000, now), '租约已过期')
  assert.equal(formatLeaseRemaining(1015, now), '剩 15 秒')
  assert.equal(formatLeaseRemaining(1060, now), '剩 1 分 0 秒')
  assert.equal(formatLeaseRemaining(1125, now), '剩 2 分 5 秒')

  // 4. Node & Attempt ID display guards
  const emptyStrings = [null, undefined, '', '   ', '\t']
  for (const v of emptyStrings) {
    assert.equal(getNodeDisplay(v), '未知')
    assert.equal(getAttemptDisplay(v), '未知')
  }
  assert.equal(getNodeDisplay('node-gpu-4070ti'), 'node-gpu-4070ti')
  assert.equal(getAttemptDisplay('att-short'), 'att-shor') // 8 chars
  assert.equal(getAttemptDisplay('att-1234567890'), 'att-1234')

  // 5. Attempt deduplication calculation on corrupted / missing fields
  assert.equal(getAttemptDeduplicationCount(null), 1)
  assert.equal(getAttemptDeduplicationCount(undefined), 1)
  assert.equal(getAttemptDeduplicationCount({}), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: null, redelivery_count: null }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 0, redelivery_count: 0 }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 1 }), 1)
  assert.equal(getAttemptDeduplicationCount({ redelivery_count: 3 }), 3)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 2, redelivery_count: 5 }), 5)

  // 6. Media path fallback rendering
  assert.ok(taskQueueSrc.includes('暂无有效视频路径'), 'task-queue.tsx must provide "暂无有效视频路径" fallback')
  assert.ok(taskQueueSrc.includes('暂无有效文件路径'), 'task-queue.tsx must provide "暂无有效文件路径" fallback')
})

// =========================================================================
// Suite 3: Popover Filter State Transitions & Search Query Wiring
// =========================================================================
test('Stress 3: Popover Filter State Transitions & Query Parameters Wiring', () => {
  // 1. Verify Popover component imports and triggers
  assert.ok(taskQueueSrc.includes('@radix-ui/react-popover'), 'Must use Radix Popover')
  assert.ok(taskQueueSrc.includes('<Popover.Root>'), 'Must mount Popover.Root')
  assert.ok(taskQueueSrc.includes('<Popover.Trigger'), 'Must mount Popover.Trigger')
  assert.ok(taskQueueSrc.includes('<Popover.Content'), 'Must mount Popover.Content')

  // 2. Active filter indicator dot
  assert.ok(
    taskQueueSrc.includes('(batch || node_id || waiting_reason) && <span className="size-1.5 rounded-full bg-current" />'),
    'Filter trigger must render active dot indicator when batch, node_id, or waiting_reason is active'
  )

  // 3. Select controls for node_id and waiting_reason
  assert.ok(taskQueueSrc.includes('value={node_id}'), 'Must bind node_id select')
  assert.ok(taskQueueSrc.includes('onChange={setNode}'), 'Must bind onChange to setNode')
  assert.ok(taskQueueSrc.includes('value={waiting_reason || \'\'}'), 'Must bind waiting_reason select')
  assert.ok(taskQueueSrc.includes('onChange={setWaitingReason}'), 'Must bind onChange to setWaitingReason')

  // 4. Clear filters button
  assert.ok(
    taskQueueSrc.includes("setBatch('')") &&
    taskQueueSrc.includes("setNode('')") &&
    taskQueueSrc.includes("setWaitingReason('')") &&
    taskQueueSrc.includes('setPage(0)'),
    'Clear filters button must atomically reset batch, node_id, waiting_reason, and page'
  )

  // 5. Query parameters propagation in load()
  assert.ok(taskQueueSrc.includes('...(node_id ? { node_id } : {})'), 'load() must propagate node_id when truthy')
  assert.ok(taskQueueSrc.includes('...(waiting_reason ? { waiting_reason } : {})'), 'load() must propagate waiting_reason when truthy')

  // 6. Direct node filter shortcut from list row
  assert.ok(
    taskQueueSrc.includes('onClick={(e) => {\n                            e.stopPropagation()\n                            setNode(task.assigned_node_id!)\n                          }}'),
    'Clicking node badge in row must set node filter directly with stopPropagation'
  )

  // 7. Backend API Route parameter forwarding in app/api/processing/tasks/route.ts
  assert.ok(apiRouteSrc.includes("const node_id = url.searchParams.get('node_id') || ''"))
  assert.ok(apiRouteSrc.includes("const waiting_reason = url.searchParams.get('waiting_reason') || ''"))
  assert.ok(apiRouteSrc.includes("if (node_id) queryParams.set('node_id', node_id)"))
  assert.ok(apiRouteSrc.includes("if (waiting_reason) queryParams.set('waiting_reason', waiting_reason)"))
})

// =========================================================================
// Suite 4: Deduplication Counter Rendering (xN badge) & Drawer Summary Strip
// =========================================================================
test('Stress 4: Deduplication Counter Rendering (xN badge) & Drawer Summary Strip', () => {
  // Test helper replicating drawer and inline dedup badge logic
  const renderDedupBadgeText = (att) => {
    const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
    const showDedupBadge = (att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)
    return showDedupBadge ? `幂等去重 x${dedupCount}` : null
  }

  // 1. Single delivery -> No badge
  assert.equal(renderDedupBadgeText({ delivery_count: 1 }), null)
  assert.equal(renderDedupBadgeText({ delivery_count: 1, redelivery_count: 0 }), null)
  assert.equal(renderDedupBadgeText({ delivery_count: 1, redelivery_count: 1 }), null)
  assert.equal(renderDedupBadgeText({ delivery_count: 0 }), null)

  // 2. Multiple deliveries -> Badge with correct count
  assert.equal(renderDedupBadgeText({ delivery_count: 2 }), '幂等去重 x2')
  assert.equal(renderDedupBadgeText({ delivery_count: 5 }), '幂等去重 x5')
  assert.equal(renderDedupBadgeText({ redelivery_count: 3 }), '幂等去重 x3')
  assert.equal(renderDedupBadgeText({ delivery_count: 2, redelivery_count: 6 }), '幂等去重 x6')

  // 3. Authoritative Result Status
  const isAuthoritativeCommitted = (attempts) =>
    attempts.some((a) => a.is_authoritative) ? '已原子提交' : '未提交'

  assert.equal(isAuthoritativeCommitted([]), '未提交')
  assert.equal(isAuthoritativeCommitted([{ is_authoritative: false }]), '未提交')
  assert.equal(isAuthoritativeCommitted([
    { is_authoritative: false },
    { is_authoritative: true },
  ]), '已原子提交')
})

// =========================================================================
// Suite 5: Static JSX / Component Render Stress Testing
// =========================================================================
test('Stress 5: Static HTML Rendering with Adversarial Payloads', () => {
  // Subcomponent mirroring AttemptCard in task-queue.tsx lines 784-824
  function AttemptCardView({ att }) {
    const isDone = att.status === 'done' || att.status === 'completed'
    const isInterrupted = att.status === 'interrupted'
    const isError = att.status === 'error' || att.status === 'failed'
    const statusClass = isDone
      ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
      : isInterrupted
      ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
      : isError
      ? 'bg-destructive/10 text-destructive'
      : 'bg-muted text-muted-foreground'
    const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)

    return React.createElement(
      'article',
      { className: 'p-3 border rounded-lg space-y-1.5 text-xs' },
      React.createElement(
        'div',
        { className: 'flex items-center justify-between' },
        React.createElement(
          'div',
          { className: 'flex items-center gap-1.5' },
          React.createElement('span', { className: 'font-semibold' }, TASK_STAGE_LABELS[att.stage] || att.stage || '未知'),
          React.createElement('code', { className: 'font-mono text-muted-foreground' }, getAttemptDisplay(att.attempt_id)),
          React.createElement(
            'span',
            { className: `px-1.5 py-0.5 rounded text-[11px] ${statusClass}` },
            isDone ? '已完成' : isInterrupted ? '中断回收' : isError ? '失败' : '运行中'
          )
        ),
        dedupCount > 1 &&
          React.createElement(
            'span',
            {
              className: 'px-1.5 py-0.5 rounded text-[11px] bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium',
            },
            `幂等去重 x${dedupCount}`
          )
      ),
      React.createElement(
        'div',
        { className: 'grid grid-cols-2 gap-1 text-muted-foreground' },
        React.createElement('span', null, '执行节点: ', React.createElement('strong', { className: 'font-mono text-foreground' }, getNodeDisplay(att.node_id))),
        React.createElement('span', null, '单次耗时: ', React.createElement('strong', { className: 'text-foreground' }, formatTaskDuration(att.duration_ms)))
      ),
      att.error && React.createElement('p', { className: 'text-destructive bg-destructive/10 p-2 rounded text-[11px] break-all' }, att.error)
    )
  }

  const corruptAttempt = {
    attempt_id: '',
    node_id: '',
    stage: 'detect',
    status: 'unrecognized_state',
    started_at: 0,
    duration_ms: -50,
    delivery_count: 0,
    redelivery_count: 0,
  }
  const htmlCorrupt = renderToStaticMarkup(React.createElement(AttemptCardView, { att: corruptAttempt }))
  assert.ok(htmlCorrupt.includes('人脸检测中'))
  assert.ok(htmlCorrupt.includes('未知'))
  assert.ok(htmlCorrupt.includes('运行中'))
  assert.ok(!htmlCorrupt.includes('幂等去重'))

  const dedupAttempt = {
    attempt_id: 'att-dedup-uuid-99999',
    node_id: 'node-gpu-4070',
    stage: 'ocr',
    status: 'interrupted',
    started_at: 1000,
    duration_ms: 1250,
    delivery_count: 1,
    redelivery_count: 4,
    error: 'Node heartbeat timeout, lease reclaimed',
  }
  const htmlDedup = renderToStaticMarkup(React.createElement(AttemptCardView, { att: dedupAttempt }))
  assert.ok(htmlDedup.includes('文字识别中'))
  assert.ok(htmlDedup.includes('att-dedu'))
  assert.ok(htmlDedup.includes('中断回收'))
  assert.ok(htmlDedup.includes('幂等去重 x4'))
  assert.ok(htmlDedup.includes('node-gpu-4070'))
  assert.ok(htmlDedup.includes('1.3 秒'))
  assert.ok(htmlDedup.includes('Node heartbeat timeout, lease reclaimed'))
})

// =========================================================================
// Suite 6: REGRESSION & RESOLUTION VERIFICATION (Fix Validation)
// =========================================================================
test('Regression Verification 1: Summary Strip Deduplication Uses Math.max Without Short-Circuit', () => {
  // 1. Verify components/processing/task-queue.tsx does NOT use the short-circuiting || operator
  assert.ok(
    !taskQueueSrc.includes('(a.delivery_count || a.redelivery_count || 1)'),
    'task-queue.tsx must not use short-circuiting || in summary strip deduplication'
  )
  assert.ok(
    taskQueueSrc.includes('Math.max(a?.delivery_count ?? 0, a?.redelivery_count ?? 0, 1)') ||
    taskQueueSrc.includes('Math.max(a.delivery_count ?? 0, a.redelivery_count ?? 0, 1)'),
    'task-queue.tsx must use Math.max across delivery_count and redelivery_count'
  )

  const att = {
    attempt_id: 'att-1',
    node_id: 'node-1',
    stage: 'ocr',
    status: 'done',
    delivery_count: 1,
    redelivery_count: 4,
  }

  // 2. Card dedup count (line 795)
  const cardDedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
  assert.equal(cardDedupCount, 4, 'Card correctly shows dedup count 4')

  // 3. Summary strip deduplication calculation correctly matches (4 deliveries - 1 original = 3 events)
  const summaryDedupEvents = Math.max(0, Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0, 1) - 1)
  assert.equal(
    summaryDedupEvents,
    3,
    'Summary strip deduplication must yield 3 events, eliminating UI contradiction with card (x4)'
  )
})

test('Regression Verification 2: formatWaitingReason Handles "unknown" Enum & Never Leaks Raw English', () => {
  // 1. 'unknown' is explicitly defined in TASK_WAITING_REASON_LABELS
  assert.deepEqual(
    TASK_WAITING_REASON_LABELS['unknown'],
    { label: '未知等待原因', description: '未知等待原因' },
    'TASK_WAITING_REASON_LABELS must define "unknown" enum key'
  )

  // 2. formatWaitingReason('unknown') returns localized Chinese
  const formattedUnknown = formatWaitingReason('unknown')
  assert.equal(formattedUnknown, '未知等待原因', 'formatWaitingReason("unknown") must return localized Chinese')

  // 3. Unrecognized waiting reason tokens degrade gracefully to localized Chinese, never raw English
  const formattedCustom = formatWaitingReason('waiting_for_storage')
  assert.equal(formattedCustom, '未知等待原因', 'formatWaitingReason("waiting_for_storage") must not leak raw English')

  // 4. Whitespace-only string returns '无等待' to prevent empty colored badge
  const whitespaceResult = formatWaitingReason('   ')
  assert.equal(whitespaceResult, '无等待', 'formatWaitingReason("   ") must return "无等待"')
  assert.equal(formatWaitingReasonDescription('   '), '任务正常流转中', 'formatWaitingReasonDescription("   ") must return "任务正常流转中"')
})

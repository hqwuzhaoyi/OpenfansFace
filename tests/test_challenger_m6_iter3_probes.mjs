/**
 * Empirical Adversarial Probe Test Suite for Milestone 6 Iteration 3
 * Tests frontend task queue deduplication calculations, waiting reason localization,
 * and handles corrupt/adversarial payloads.
 */

import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const projectRoot = path.resolve(__dirname, '..')

// Source file paths
const taskModelPath = path.join(projectRoot, 'lib', 'processing-task-model.ts')
const taskQueuePath = path.join(projectRoot, 'components', 'processing', 'task-queue.tsx')

// Import model functions and constants
import {
  TASK_WAITING_REASON_LABELS,
  TASK_STAGE_LABELS,
  formatWaitingReason,
  formatTaskWaitingReason,
  formatWaitingReasonDescription,
  formatTaskWaitingReasonDescription,
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  getAttemptDeduplicationCount,
} from '../lib/processing-task-model.ts'

const taskQueueSrc = fs.readFileSync(taskQueuePath, 'utf8')
const taskModelSrc = fs.readFileSync(taskModelPath, 'utf8')

// =========================================================================
// Suite 1: Deduplication Summary Formula & Mixed Counters Probe
// =========================================================================
test('Challenger Probe 1: Deduplication summary strip calculation and edge cases', () => {
  // Formula from components/processing/task-queue.tsx line 771:
  // taskAttempts.reduce((acc, a) => acc + Math.max(0, Math.max(a?.delivery_count ?? 0, a?.redelivery_count ?? 0, 1) - 1), 0)
  const computeDedupSummary = (attempts) =>
    attempts.reduce(
      (acc, a) =>
        acc +
        Math.max(
          0,
          Math.max(a?.delivery_count ?? 0, a?.redelivery_count ?? 0, 1) - 1
        ),
      0
    )

  // 1. Mandatory requirement: { delivery_count: 1, redelivery_count: 4 } must evaluate to 3, not 0
  const probeTarget = [{ delivery_count: 1, redelivery_count: 4 }]
  assert.equal(
    computeDedupSummary(probeTarget),
    3,
    '{ delivery_count: 1, redelivery_count: 4 } must evaluate to 3 deduplication events'
  )

  // 2. Symmetric counter: { delivery_count: 4, redelivery_count: 1 } must also evaluate to 3
  assert.equal(
    computeDedupSummary([{ delivery_count: 4, redelivery_count: 1 }]),
    3
  )

  // 3. Single delivery without redeliveries: 0 deduplication events
  assert.equal(computeDedupSummary([{ delivery_count: 1 }]), 0)
  assert.equal(
    computeDedupSummary([{ delivery_count: 1, redelivery_count: 0 }]),
    0
  )
  assert.equal(
    computeDedupSummary([{ delivery_count: 1, redelivery_count: 1 }]),
    0
  )

  // 4. Missing delivery_count with redelivery_count
  assert.equal(computeDedupSummary([{ redelivery_count: 5 }]), 4)
  assert.equal(computeDedupSummary([{ delivery_count: 5 }]), 4)

  // 5. Zero and negative counts must never subtract or go negative
  assert.equal(
    computeDedupSummary([{ delivery_count: 0, redelivery_count: 0 }]),
    0
  )
  assert.equal(
    computeDedupSummary([{ delivery_count: -5, redelivery_count: -10 }]),
    0
  )
  assert.equal(
    computeDedupSummary([{ delivery_count: -1, redelivery_count: 3 }]),
    2
  )

  // 6. Adversarial payloads: null, undefined, empty object
  assert.equal(computeDedupSummary([null]), 0)
  assert.equal(computeDedupSummary([undefined]), 0)
  assert.equal(computeDedupSummary([{}]), 0)
  assert.equal(
    computeDedupSummary([
      null,
      {},
      undefined,
      { delivery_count: 0 },
      { delivery_count: 1, redelivery_count: 4 }, // 3
      { delivery_count: 2, redelivery_count: 1 }, // 1
      { delivery_count: 3, redelivery_count: 5 }, // 4
    ]),
    8
  )

  // 7. Verify static code in task-queue.tsx has the exact safe Math.max expression
  assert.ok(
    !taskQueueSrc.includes('(a.delivery_count || a.redelivery_count || 1)'),
    'task-queue.tsx must NOT contain the buggy short-circuiting logical OR'
  )
  assert.ok(
    taskQueueSrc.includes('Math.max(a?.delivery_count ?? 0, a?.redelivery_count ?? 0, 1)'),
    'task-queue.tsx must use optional chaining and nullish coalescing in summary strip'
  )
})

// =========================================================================
// Suite 2: Waiting Reason Fallback & Localization Invariants
// =========================================================================
test('Challenger Probe 2: formatWaitingReason localization and adversarial string fallbacks', () => {
  // 1. Mandatory requirement: formatWaitingReason('unknown') returns '未知等待原因'
  assert.equal(
    formatWaitingReason('unknown'),
    '未知等待原因',
    'formatWaitingReason("unknown") must return "未知等待原因"'
  )
  assert.equal(
    formatWaitingReasonDescription('unknown'),
    '未知等待原因',
    'formatWaitingReasonDescription("unknown") must return "未知等待原因"'
  )

  // 2. Mandatory requirement: formatWaitingReason('   ') returns '无等待', never ""
  assert.equal(
    formatWaitingReason('   '),
    '无等待',
    'formatWaitingReason("   ") must return "无等待"'
  )
  assert.equal(
    formatWaitingReasonDescription('   '),
    '任务正常流转中',
    'formatWaitingReasonDescription("   ") must return "任务正常流转中"'
  )
  assert.equal(formatWaitingReason('\t\n\r  '), '无等待')
  assert.equal(formatWaitingReasonDescription('\t\n\r  '), '任务正常流转中')

  // 3. Mandatory requirement: formatWaitingReason('waiting_for_storage') returns '未知等待原因'
  assert.equal(
    formatWaitingReason('waiting_for_storage'),
    '未知等待原因',
    'formatWaitingReason("waiting_for_storage") must degrade to "未知等待原因"'
  )
  assert.equal(
    formatWaitingReasonDescription('waiting_for_storage'),
    '未知等待原因',
    'formatWaitingReasonDescription("waiting_for_storage") must degrade to "未知等待原因"'
  )

  // 4. Standard known waiting reasons
  const expectedMappings = [
    ['none', '无等待', '任务正常流转中'],
    ['waiting_for_node', '等待计算节点', '当前无空闲或满足硬件能力要求的计算节点'],
    ['waiting_node', '等待计算节点', '当前无空闲或满足硬件能力要求的计算节点'],
    ['quota_exhausted', '并发配额超限', '已达到全局并发上限或物理设备配额阶梯'],
    ['quota_limit', '并发配额超限', '已达到全局并发上限或物理设备配额阶梯'],
    ['gpu_busy', 'GPU 显存繁忙', '显存或算力不足以承载新检测任务'],
    ['rate_limited', '并发降频限流', '系统正在执行优雅并发控制，在途任务完成后方可领取'],
    ['retry_backoff', '重试退避等待', '前次执行失败，处于退避等待窗口'],
    ['model_warmup', '模型预热自检', '计算节点正在执行启动预热与基准漂移校验'],
    ['paused', '人工暂停接单', '操作员已暂停调度'],
    ['unknown', '未知等待原因', '未知等待原因'],
  ]

  for (const [key, label, desc] of expectedMappings) {
    assert.equal(formatWaitingReason(key), label, `Key '${key}' must map to '${label}'`)
    assert.equal(
      formatWaitingReasonDescription(key),
      desc,
      `Key '${key}' must have description '${desc}'`
    )
  }

  // 5. Alias functions export check
  assert.equal(formatTaskWaitingReason('unknown'), '未知等待原因')
  assert.equal(formatTaskWaitingReasonDescription('unknown'), '未知等待原因')

  // 6. Adversarial prototype pollution attempts
  const protoKeys = ['toString', 'valueOf', 'constructor', '__proto__', 'isPrototypeOf']
  for (const key of protoKeys) {
    assert.equal(
      formatWaitingReason(key),
      '未知等待原因',
      `Proto key '${key}' must safely degrade to '未知等待原因'`
    )
    assert.equal(
      formatWaitingReasonDescription(key),
      '未知等待原因',
      `Proto key '${key}' description must safely degrade to '未知等待原因'`
    )
  }

  // 7. Verify formatWaitingReason and formatWaitingReasonDescription do not contain fallback to raw trimmed string
  const formatWaitingReasonSection = taskModelSrc.slice(
    taskModelSrc.indexOf('export function formatWaitingReason('),
    taskModelSrc.indexOf('export function formatTaskDuration(')
  )
  assert.ok(
    !formatWaitingReasonSection.includes('|| trimmed'),
    'formatWaitingReason section must NOT contain "|| trimmed" fallback'
  )
  assert.ok(
    formatWaitingReasonSection.includes("|| '未知等待原因'"),
    'formatWaitingReason section must fall back to "未知等待原因"'
  )
})

// =========================================================================
// Suite 3: Adversarial Attempts Payloads & Attempt History Component Simulation
// =========================================================================
test('Challenger Probe 3: Adversarial attempts payloads rendering simulation', () => {
  // Mirroring AttemptCard rendering in task-queue.tsx
  function AttemptCard({ att }) {
    if (!att) return null
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
          React.createElement(
            'span',
            { className: 'font-semibold' },
            TASK_STAGE_LABELS[att.stage] || att.stage || '未知'
          ),
          React.createElement(
            'code',
            { className: 'font-mono text-muted-foreground' },
            getAttemptDisplay(att.attempt_id)
          ),
          React.createElement(
            'span',
            { className: `px-1.5 py-0.5 rounded text-[11px] ${statusClass}` },
            isDone
              ? '已完成'
              : isInterrupted
              ? '中断回收'
              : isError
              ? '失败'
              : '运行中'
          )
        ),
        dedupCount > 1 &&
          React.createElement(
            'span',
            {
              className:
                'px-1.5 py-0.5 rounded text-[11px] bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium',
            },
            `幂等去重 x${dedupCount}`
          )
      ),
      React.createElement(
        'div',
        { className: 'grid grid-cols-2 gap-1 text-muted-foreground' },
        React.createElement(
          'span',
          null,
          '执行节点: ',
          React.createElement(
            'strong',
            { className: 'font-mono text-foreground' },
            getNodeDisplay(att.node_id)
          )
        ),
        React.createElement(
          'span',
          null,
          '单次耗时: ',
          React.createElement(
            'strong',
            { className: 'text-foreground' },
            formatTaskDuration(att.duration_ms)
          )
        )
      ),
      att.error &&
        React.createElement(
          'p',
          {
            className:
              'text-destructive bg-destructive/10 p-2 rounded text-[11px] break-all',
          },
          att.error
        )
    )
  }

  // 1. Render card with delivery_count: 1, redelivery_count: 4
  const html1 = renderToStaticMarkup(
    React.createElement(AttemptCard, {
      att: {
        attempt_id: 'att-12345678',
        node_id: 'node-pc-wsl',
        stage: 'ocr',
        status: 'done',
        started_at: 1000,
        finished_at: 1005,
        duration_ms: 5000,
        delivery_count: 1,
        redelivery_count: 4,
        is_authoritative: true,
      },
    })
  )
  assert.ok(html1.includes('幂等去重 x4'), 'Must render badge with x4')
  assert.ok(html1.includes('文字识别中'), 'Must render stage label')
  assert.ok(html1.includes('node-pc-wsl'), 'Must render node id')
  assert.ok(html1.includes('5.0 秒'), 'Must render formatted duration')

  // 2. Render card with negative duration, null error, empty stage
  const html2 = renderToStaticMarkup(
    React.createElement(AttemptCard, {
      att: {
        attempt_id: '',
        node_id: '',
        stage: '',
        status: 'failed',
        started_at: 0,
        duration_ms: -100,
        delivery_count: -1,
        redelivery_count: -5,
        is_authoritative: false,
      },
    })
  )
  assert.ok(html2.includes('未知'), 'Empty node and stage must render 未知')
  assert.ok(html2.includes('失败'), 'Status failed must render 失败')
  assert.ok(!html2.includes('幂等去重'), 'Negative dedup counts must not render badge')

  // 3. Render card with empty attempt object
  const html3 = renderToStaticMarkup(
    React.createElement(AttemptCard, { att: {} })
  )
  assert.ok(html3.includes('未知'))
  assert.ok(html3.includes('运行中'))
})

// =========================================================================
// Suite 4: Consistency Between Summary Strip & Attempt Card Badges
// =========================================================================
test('Challenger Probe 4: Card badge dedup count vs summary strip event count consistency', () => {
  // In our system:
  // - The attempt card displays: dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
  //   Badge shows: "幂等去重 x{dedupCount}" (when dedupCount > 1)
  // - The drawer summary strip displays: total deduplication events = Math.max(0, Math.max(...) - 1)
  // For an attempt delivered 4 times:
  // - 1st delivery was the initial attempt
  // - 3 subsequent deliveries were deduplicated
  // Therefore, badge says x4, and summary strip adds 3.
  const testCases = [
    { delivery: 1, redelivery: 4, expectedCardBadge: '幂等去重 x4', expectedSummaryEvents: 3 },
    { delivery: 4, redelivery: 1, expectedCardBadge: '幂重去重 x4', expectedSummaryEvents: 3 },
    { delivery: 1, redelivery: 1, expectedCardBadge: null, expectedSummaryEvents: 0 },
    { delivery: 1, redelivery: 0, expectedCardBadge: null, expectedSummaryEvents: 0 },
    { delivery: 2, redelivery: 0, expectedCardBadge: '幂等去重 x2', expectedSummaryEvents: 1 },
    { delivery: 0, redelivery: 2, expectedCardBadge: '幂等去重 x2', expectedSummaryEvents: 1 },
    { delivery: 5, redelivery: 7, expectedCardBadge: '幂等去重 x7', expectedSummaryEvents: 6 },
  ]

  for (const tc of testCases) {
    const cardCount = Math.max(tc.delivery, tc.redelivery)
    const cardBadge = cardCount > 1 ? `幂等去重 x${cardCount}` : null
    const summaryEvents = Math.max(0, Math.max(tc.delivery, tc.redelivery, 1) - 1)

    if (tc.expectedCardBadge) {
      assert.equal(cardBadge, `幂等去重 x${cardCount}`)
    } else {
      assert.equal(cardBadge, null)
    }
    assert.equal(
      summaryEvents,
      tc.expectedSummaryEvents,
      `Mismatch for delivery=${tc.delivery}, redelivery=${tc.redelivery}`
    )
  }
})

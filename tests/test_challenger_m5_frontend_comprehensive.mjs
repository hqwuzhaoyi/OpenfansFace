import { test } from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
} from '../lib/processing-task-model.ts'

// Replicate the exact rendering logic in components/processing/task-queue.tsx lines 294-325
function renderTaskAttemptItem(att) {
  const isDone = att.status === 'done' || att.status === 'completed'
  const isInterrupted = att.status === 'interrupted'
  const isError = att.status === 'error' || att.status === 'failed'
  const statusSpanClass = `px-1.5 py-0.5 rounded text-[11px] font-sans ${
    isDone
      ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
      : isInterrupted
      ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
      : isError
      ? 'bg-destructive/10 text-destructive'
      : 'bg-muted text-muted-foreground'
  }`

  const statusLabel = isDone
    ? '已完成'
    : isInterrupted
    ? '中断回收'
    : isError
    ? '失败'
    : '运行中'

  const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
  const hasDedupBadge = (att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)
  const dedupBadgeText = hasDedupBadge ? `幂等去重 x${dedupCount}` : null

  return {
    stage: att.stage,
    attemptDisplay: getAttemptDisplay(att.attempt_id),
    statusSpanClass,
    statusLabel,
    hasDedupBadge,
    dedupBadgeText,
    durationText: typeof att.duration_ms === 'number' ? `(${formatTaskDuration(att.duration_ms)})` : null,
  }
}

test('Empirical Test 1: Status Synonym Handling ("completed", "failed", "interrupted", "running")', () => {
  // Test native statuses that pass:
  const doneResult = renderTaskAttemptItem({ status: 'done', delivery_count: 1 })
  assert.equal(doneResult.statusLabel, '已完成')
  assert.ok(doneResult.statusSpanClass.includes('emerald'))

  const interruptedResult = renderTaskAttemptItem({ status: 'interrupted', delivery_count: 1 })
  assert.equal(interruptedResult.statusLabel, '中断回收')
  assert.ok(interruptedResult.statusSpanClass.includes('amber'))

  const errorResult = renderTaskAttemptItem({ status: 'error', delivery_count: 1 })
  assert.equal(errorResult.statusLabel, '失败')
  assert.ok(errorResult.statusSpanClass.includes('destructive'))

  const runningResult = renderTaskAttemptItem({ status: 'running', delivery_count: 1 })
  assert.equal(runningResult.statusLabel, '运行中')
  assert.ok(runningResult.statusSpanClass.includes('muted'))

  // Positive verification: status synonyms 'completed' and 'failed'
  const completedResult = renderTaskAttemptItem({ status: 'completed', delivery_count: 1 })
  const failedResult = renderTaskAttemptItem({ status: 'failed', delivery_count: 1 })

  assert.equal(completedResult.statusLabel, '已完成', 'status="completed" must normalize to "已完成"')
  assert.ok(completedResult.statusSpanClass.includes('emerald'), 'status="completed" must have emerald class')
  assert.equal(failedResult.statusLabel, '失败', 'status="failed" must normalize to "失败"')
  assert.ok(failedResult.statusSpanClass.includes('destructive'), 'status="failed" must have destructive class')
})

test('Empirical Test 2: Deduplication Badge with delivery_count vs redelivery_count', () => {
  // Case A: Single delivery (delivery_count: 1) -> No deduplication badge
  const single = renderTaskAttemptItem({ delivery_count: 1 })
  assert.equal(single.hasDedupBadge, false)
  assert.equal(single.dedupBadgeText, null)

  // Case B: Explicit delivery_count: 3 -> Deduplication badge appears properly
  const multiDelivery = renderTaskAttemptItem({ delivery_count: 3 })
  assert.equal(multiDelivery.hasDedupBadge, true)
  assert.equal(multiDelivery.dedupBadgeText, '幂等去重 x3')

  // Case C: Provided with redelivery_count: 3 (without delivery_count)
  const multiRedeliveryOnly = renderTaskAttemptItem({ redelivery_count: 3 })
  assert.equal(multiRedeliveryOnly.hasDedupBadge, true, 'redelivery_count: 3 must trigger deduplication badge')
  assert.equal(multiRedeliveryOnly.dedupBadgeText, '幂等去重 x3', 'redelivery_count: 3 must render 幂等去重 x3')

  // Case D: Provided with delivery_count: 1 and redelivery_count: 3
  const multiBoth = renderTaskAttemptItem({ delivery_count: 1, redelivery_count: 3 })
  assert.equal(multiBoth.hasDedupBadge, true, 'delivery_count: 1 with redelivery_count: 3 must trigger deduplication badge')
  assert.equal(multiBoth.dedupBadgeText, '幂等去重 x3', 'deduplication badge must use max(delivery_count, redelivery_count)')
})

test('Empirical Test 3: Truth-in-UI Invariants (Never "0s" or Blank)', () => {
  // Durations
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')
  assert.equal(formatTaskDuration(0), '未知')
  assert.equal(formatTaskDuration(-1), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')

  // Nodes
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(undefined), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getNodeDisplay('   '), '未知')

  // Attempts
  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(undefined), '未知')
  assert.equal(getAttemptDisplay(''), '未知')
  assert.equal(getAttemptDisplay('   '), '未知')

  // Lease remaining
  const now = 1000
  assert.equal(formatLeaseRemaining(null, now), '无有效租约')
  assert.equal(formatLeaseRemaining(0, now), '无有效租约')
  assert.equal(formatLeaseRemaining(-1, now), '无有效租约')
  assert.equal(formatLeaseRemaining(now - 10, now), '租约已过期')
})

test('Empirical Test 4: Detail Drawer Focus Invariant During 5s Polling Cycles', () => {
  const updateFocused = (previous, resp) =>
    previous
      ? (resp.task == null
        ? resp.tasks.find((t) => t.vid === previous.vid) || previous
        : resp.task.vid === previous.vid
          ? resp.task
          : resp.tasks.find((t) => t.vid === previous.vid) || previous)
      : null

  let focused = {
    vid: 'target-vid-007',
    name: 'important_video.mp4',
    stage: 'text',
    assigned_node_id: 'node-pc-1',
    attempt_id: 'att-007',
  }

  // Poll 1: Task in list, stage progresses to detect
  focused = updateFocused(focused, {
    page: 0,
    tasks: [{ vid: 'target-vid-007', name: 'important_video.mp4', stage: 'detect' }],
    task: null,
  })
  assert.equal(focused.stage, 'detect')

  // Poll 2: User filtered or navigated; task is paged out (not in tasks list)
  focused = updateFocused(focused, {
    page: 2,
    tasks: [{ vid: 'other-1', name: 'other.mp4', stage: 'done' }],
    task: null,
  })
  assert.equal(focused.vid, 'target-vid-007', 'Must retain focused task when paged out')
  assert.equal(focused.stage, 'detect', 'Must retain existing attributes')

  // Poll 3: Backend returns task via dedicated query param
  focused = updateFocused(focused, {
    page: 2,
    tasks: [],
    task: { vid: 'target-vid-007', name: 'important_video.mp4', stage: 'done' },
  })
  assert.equal(focused.stage, 'done')

  // Poll 4: Backend returns unrelated task due to race condition
  focused = updateFocused(focused, {
    page: 0,
    tasks: [],
    task: { vid: 'race-condition-task', name: 'race.mp4' },
  })
  assert.equal(focused.vid, 'target-vid-007', 'Must not be overwritten by race condition task')
})

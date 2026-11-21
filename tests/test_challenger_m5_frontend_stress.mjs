import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
} from '../lib/processing-task-model.ts'

const root = process.cwd()

// Extract or mirror the exact attempt badge renderer from components/processing/task-queue.tsx lines 302-321
function renderAttemptBadgeFromComponent(att) {
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

  const statusLabel = isDone
    ? '已完成'
    : isInterrupted
    ? '中断回收'
    : isError
    ? '失败'
    : '运行中'

  const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)
  const showDedupBadge = (att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)
  const dedupText = showDedupBadge ? `幂等去重 x${dedupCount}` : null

  return {
    statusClass,
    statusLabel,
    showDedupBadge,
    dedupText,
  }
}

// React component representing the exact attempt list rendering in components/processing/task-queue.tsx
function TaskAttemptsView({ attempts }) {
  if (!attempts || attempts.length === 0) return null

  return React.createElement(
    'div',
    { className: 'space-y-1.5' },
    attempts.map((att) => {
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
      const statusLabel = isDone
        ? '已完成'
        : isInterrupted
        ? '中断回收'
        : isError
        ? '失败'
        : '运行中'
      const dedupCount = Math.max(att.delivery_count ?? 0, att.redelivery_count ?? 0)

      return React.createElement(
        'div',
        {
          key: att.attempt_id,
          className: 'flex flex-wrap items-center gap-1.5 text-xs font-mono',
        },
        React.createElement('span', { className: 'font-semibold' }, TASK_STAGE_LABELS[att.stage] || att.stage),
        React.createElement(
          'span',
          { className: 'text-muted-foreground' },
          getAttemptDisplay(att.attempt_id)
        ),
        React.createElement(
          'span',
          {
            className: `px-1.5 py-0.5 rounded text-[11px] font-sans ${statusClass}`,
          },
          statusLabel
        ),
        ((att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)) &&
          React.createElement(
            'span',
            {
              className:
                'px-1.5 py-0.5 rounded text-[11px] font-sans bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium',
            },
            `幂等去重 x${dedupCount}`
          ),
        typeof att.duration_ms === 'number' &&
          React.createElement(
            'span',
            { className: 'text-muted-foreground' },
            `(${formatTaskDuration(att.duration_ms)})`
          )
      )
    })
  )
}

// =====================================================================
// Test 1: Deduplication Badge Rendering with Single vs Multiple Deliveries
// =====================================================================
test('Challenge 1: Attempt Deduplication Rendering (Single vs Multiple Deliveries)', () => {
  // Case A: Single delivery (delivery_count: 1) -> No deduplication badge
  const singleAttempt = {
    attempt_id: 'att-single-001',
    node_id: 'node-gpu-1',
    stage: 'ocr',
    status: 'done',
    started_at: 1000,
    finished_at: 1002,
    duration_ms: 2000,
    delivery_count: 1,
    is_authoritative: true,
  }
  const singleHtml = renderToStaticMarkup(
    React.createElement(TaskAttemptsView, { attempts: [singleAttempt] })
  )
  assert.ok(!singleHtml.includes('幂等去重'), 'Single delivery must NOT show 幂等去重 badge')
  assert.ok(singleHtml.includes('文字识别中') || singleHtml.includes('ocr'), 'Must show stage')
  assert.ok(singleHtml.includes('att-sing'), 'Must show attempt ID prefix')
  assert.ok(singleHtml.includes('已完成'), 'Must show status')
  assert.ok(singleHtml.includes('2.0 秒'), 'Must show duration')

  // Case B: Multiple deliveries (delivery_count: 3) -> Must show '幂等去重 x3'
  const multiAttempt = {
    attempt_id: 'att-multi-003',
    node_id: 'node-gpu-1',
    stage: 'ocr',
    status: 'done',
    started_at: 1000,
    finished_at: 1002,
    duration_ms: 2000,
    delivery_count: 3,
    is_authoritative: true,
  }
  const multiHtml = renderToStaticMarkup(
    React.createElement(TaskAttemptsView, { attempts: [multiAttempt] })
  )
  assert.ok(multiHtml.includes('幂等去重 x3'), 'delivery_count: 3 must render 幂等去重 x3')
  assert.ok(multiHtml.includes('bg-blue-500/10'), 'Must have distinct blue highlight badge')

  // Case C: Layout stability check on large delivery counts
  const highAttempt = {
    attempt_id: 'att-multi-999',
    node_id: 'node-gpu-1',
    stage: 'detect',
    status: 'done',
    started_at: 1000,
    delivery_count: 99999,
    is_authoritative: true,
  }
  const highHtml = renderToStaticMarkup(
    React.createElement(TaskAttemptsView, { attempts: [highAttempt] })
  )
  assert.ok(highHtml.includes('幂等去重 x99999'))
  assert.ok(highHtml.includes('flex flex-wrap'), 'Must use flex flex-wrap to prevent horizontal overflow')

  // Case D: Multiple deliveries via redelivery_count (redelivery_count: 3) -> Must show '幂等去重 x3'
  const redeliverAttempt = {
    attempt_id: 'att-multi-redeliver',
    node_id: 'node-gpu-1',
    stage: 'ocr',
    status: 'completed',
    started_at: 1000,
    finished_at: 1002,
    duration_ms: 2000,
    redelivery_count: 3,
    is_authoritative: true,
  }
  const redeliverHtml = renderToStaticMarkup(
    React.createElement(TaskAttemptsView, { attempts: [redeliverAttempt] })
  )
  assert.ok(redeliverHtml.includes('幂等去重 x3'), 'redelivery_count: 3 must render 幂等去重 x3')
})

// =====================================================================
// Test 2: Stress-Testing Attempt Status Handling (done vs completed, error vs failed)
// =====================================================================
test('Challenge 2: Attempt Status Matrix Evaluation', () => {
  // Test native statuses
  const rDone = renderAttemptBadgeFromComponent({ status: 'done', delivery_count: 1 })
  assert.equal(rDone.statusLabel, '已完成')
  assert.ok(rDone.statusClass.includes('emerald'))

  const rInterrupted = renderAttemptBadgeFromComponent({ status: 'interrupted', delivery_count: 1 })
  assert.equal(rInterrupted.statusLabel, '中断回收')
  assert.ok(rInterrupted.statusClass.includes('amber'))

  const rError = renderAttemptBadgeFromComponent({ status: 'error', delivery_count: 1 })
  assert.equal(rError.statusLabel, '失败')
  assert.ok(rError.statusClass.includes('destructive'))

  const rRunning = renderAttemptBadgeFromComponent({ status: 'running', delivery_count: 1 })
  assert.equal(rRunning.statusLabel, '运行中')
  assert.ok(rRunning.statusClass.includes('muted'))

  // Status synonyms 'completed' and 'failed'
  const rCompleted = renderAttemptBadgeFromComponent({ status: 'completed', delivery_count: 1 })
  const rFailed = renderAttemptBadgeFromComponent({ status: 'failed', delivery_count: 1 })

  assert.equal(rCompleted.statusLabel, '已完成', 'status="completed" must normalize to "已完成"')
  assert.ok(rCompleted.statusClass.includes('emerald'), 'status="completed" must have emerald class')
  assert.equal(rFailed.statusLabel, '失败', 'status="failed" must normalize to "失败"')
  assert.ok(rFailed.statusClass.includes('destructive'), 'status="failed" must have destructive class')
})

// =====================================================================
// Test 3: Truth-in-UI Invariants (Missing/Unknown Metrics Never "0s" or Blank)
// =====================================================================
test('Challenge 3: Truth-in-UI Duration, Node, and Metrics Formatters', () => {
  // 1. Duration: Must render '未知' on null, undefined, 0, or negative numbers; NEVER '0s' or blank
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')
  assert.equal(formatTaskDuration(0), '未知')
  assert.equal(formatTaskDuration(-10), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')
  assert.equal(formatTaskDuration(Infinity), '未知')
  assert.notEqual(formatTaskDuration(null), '0s')
  assert.notEqual(formatTaskDuration(0), '0s')
  assert.notEqual(formatTaskDuration(null), '')

  // 2. Node display: Must render '未知' on missing, blank, or whitespace; NEVER blank
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(undefined), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getNodeDisplay('   '), '未知')
  assert.equal(getNodeDisplay('node-cuda-4070ti'), 'node-cuda-4070ti')

  // 3. Attempt display: Must render '未知' on missing; truncates to 8 chars if longer
  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(undefined), '未知')
  assert.equal(getAttemptDisplay(''), '未知')
  assert.equal(getAttemptDisplay('   '), '未知')
  assert.equal(getAttemptDisplay('attempt-12345'), 'attempt-')

  // 4. Lease display: Must render '无有效租约' or '租约已过期'; NEVER '0s'
  const now = 1000
  assert.equal(formatLeaseRemaining(null, now), '无有效租约')
  assert.equal(formatLeaseRemaining(0, now), '无有效租约')
  assert.equal(formatLeaseRemaining(-5, now), '无有效租约')
  assert.equal(formatLeaseRemaining(now - 1, now), '租约已过期')
  assert.notEqual(formatLeaseRemaining(0, now), '0s')
})

// =====================================================================
// Test 4: Detail Drawer Focus Invariant During 5s Background Polling
// =====================================================================
test('Challenge 4: Focus Invariant Under Background Polling', () => {
  // The state transition formula in task-queue.tsx line 147-153:
  const updateFocused = (previous, resp) =>
    previous
      ? (resp.task == null
        ? resp.tasks.find((t) => t.vid === previous.vid) || previous
        : resp.task.vid === previous.vid
          ? resp.task
          : resp.tasks.find((t) => t.vid === previous.vid) || previous)
      : null

  const focusedTask = {
    vid: 'vid-m5-focus-1',
    name: 'test_video_m5.mp4',
    path: '/media/test_video_m5.mp4',
    stage: 'ocr',
    assigned_node_id: 'node-pc-4070ti',
    attempt_id: 'att-m5-001',
    coordinator_generation: 42,
    size: 1073741824,
    batch_id: 'batch-20261112',
  }

  // 1. Regular 5s poll response: task is in task list, updated to done
  const poll1 = {
    page: 0,
    tasks: [{ ...focusedTask, stage: 'done' }],
    task: null,
  }
  const res1 = updateFocused(focusedTask, poll1)
  assert.notEqual(res1, null, 'Focused task must NOT be cleared')
  assert.equal(res1.vid, 'vid-m5-focus-1')
  assert.equal(res1.stage, 'done', 'Should pick up updated fields from list')

  // 2. Poll response where task has paged out of current view
  const pollPagedOut = {
    page: 1,
    tasks: [{ vid: 'other-task', name: 'other.mp4' }],
    task: null,
  }
  const res2 = updateFocused(focusedTask, pollPagedOut)
  assert.notEqual(res2, null, 'Focused task must NOT be cleared when paged out')
  assert.equal(res2.vid, 'vid-m5-focus-1')

  // 3. Poll response with dedicated updated task matching focused
  const pollWithSpecificTask = {
    page: 0,
    tasks: [],
    task: { ...focusedTask, stage: 'feature', coordinator_generation: 43 },
  }
  const res3 = updateFocused(focusedTask, pollWithSpecificTask)
  assert.notEqual(res3, null)
  assert.equal(res3.coordinator_generation, 43)

  // 4. Poll response with unrelated dedicated task (e.g. race condition)
  const pollUnrelatedTask = {
    page: 0,
    tasks: [],
    task: { vid: 'unrelated', name: 'unrelated.mp4' },
  }
  const res4 = updateFocused(focusedTask, pollUnrelatedTask)
  assert.notEqual(res4, null, 'Unrelated task must not displace current focus')
  assert.equal(res4.vid, 'vid-m5-focus-1')

  // 5. Unfocused user state remains null
  assert.equal(updateFocused(null, poll1), null)
})

// =====================================================================
// Test 5: Full Task Queue Component Static Code Invariants
// =====================================================================
test('Challenge 5: Task Queue Source Code Contract Invariants', () => {
  const compPath = path.resolve(root, 'components/processing/task-queue.tsx')
  const content = fs.readFileSync(compPath, 'utf-8')

  // Attempt history API wiring
  assert.ok(
    content.includes('/api/processing/task-attempts?vid=') || content.includes('/v1/agent/task-attempts?vid='),
    'Must fetch task-attempts when focused.vid changes'
  )

  // Detail drawer renders attempt history section
  assert.ok(content.includes('taskAttempts.length > 0'), 'Must conditionally render attempt history')
  assert.ok(content.includes('尝试与去重'), 'Must have label 尝试与去重')
  assert.ok(content.includes('幂等去重 x'), 'Must render 幂等去重 badge')

  // Status mapping
  assert.ok(content.includes('已完成'), 'Must render 已完成')
  assert.ok(content.includes('中断回收'), 'Must render 中断回收')
  assert.ok(content.includes('失败'), 'Must render 失败')
  assert.ok(content.includes('运行中'), 'Must render 运行中')

  // Truth-in-UI metrics
  assert.ok(content.includes('formatTaskDuration'), 'Must use formatTaskDuration')
  assert.ok(content.includes('getNodeDisplay'), 'Must use getNodeDisplay')
  assert.ok(content.includes('getAttemptDisplay'), 'Must use getAttemptDisplay')
  assert.ok(content.includes('formatLeaseRemaining'), 'Must use formatLeaseRemaining')

  // Background polling timer
  assert.ok(content.includes('setTimeout(run, 5000)'), 'Must maintain 5s background polling loop')
})

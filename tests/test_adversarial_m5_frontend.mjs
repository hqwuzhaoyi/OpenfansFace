import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import { spawn } from 'node:child_process'
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

const ROOT = process.cwd()

// Mirror of the exact rendering logic in components/processing/task-queue.tsx lines 302-337
function renderAttemptInTaskQueue(att) {
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
  const showDedup = (att.delivery_count > 1) || ((att.redelivery_count ?? 0) > 1)

  return React.createElement(
    'div',
    { key: att.attempt_id, className: 'flex flex-wrap items-center gap-1.5 text-xs font-mono' },
    React.createElement('span', { className: 'font-semibold' }, TASK_STAGE_LABELS[att.stage] || att.stage),
    React.createElement('span', { className: 'text-muted-foreground' }, getAttemptDisplay(att.attempt_id)),
    React.createElement(
      'span',
      { className: `px-1.5 py-0.5 rounded text-[11px] font-sans ${statusClass}` },
      statusLabel
    ),
    showDedup &&
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
}

test('Adversarial 1: Status Synonyms & Fallback Stress Test', () => {
  // Test matrix of statuses
  const cases = [
    { status: 'done', expectedLabel: '已完成', expectedClass: 'emerald' },
    { status: 'completed', expectedLabel: '已完成', expectedClass: 'emerald' },
    { status: 'interrupted', expectedLabel: '中断回收', expectedClass: 'amber' },
    { status: 'error', expectedLabel: '失败', expectedClass: 'destructive' },
    { status: 'failed', expectedLabel: '失败', expectedClass: 'destructive' },
    { status: 'running', expectedLabel: '运行中', expectedClass: 'muted' },
    { status: 'unknown_distributed_state', expectedLabel: '运行中', expectedClass: 'muted' },
    { status: '', expectedLabel: '运行中', expectedClass: 'muted' },
    { status: undefined, expectedLabel: '运行中', expectedClass: 'muted' },
  ]

  for (const c of cases) {
    const markup = renderToStaticMarkup(renderAttemptInTaskQueue({
      attempt_id: 'att-test',
      stage: 'ocr',
      status: c.status,
      delivery_count: 1,
    }))
    assert.ok(markup.includes(c.expectedLabel), `Status ${c.status} should render label ${c.expectedLabel}`)
    assert.ok(markup.includes(c.expectedClass), `Status ${c.status} should contain class ${c.expectedClass}`)
    // Confirm 'completed' and 'failed' NEVER fall back to '运行中'
    if (c.status === 'completed' || c.status === 'failed') {
      assert.ok(!markup.includes('运行中'), `Status ${c.status} must never fall back to 运行中`)
    }
  }
})

test('Adversarial 2: Deduplication Multiplier Combinations & Edge Cases', () => {
  const cases = [
    // [delivery_count, redelivery_count, shouldShow, expectedBadge]
    [1, undefined, false, null],
    [1, 0, false, null],
    [1, 1, false, null],
    [1, 2, true, '幂等去重 x2'],
    [1, 3, true, '幂等去重 x3'],
    [2, 1, true, '幂等去重 x2'],
    [3, 1, true, '幂等去重 x3'],
    [4, 5, true, '幂等去重 x5'],
    [0, 3, true, '幂等去重 x3'],
    [undefined, 4, true, '幂等去重 x4'],
    [0, 0, false, null],
    [undefined, undefined, false, null],
    [0, 1, false, null],
    [-1, -1, false, null],
    [1, 999, true, '幂等去重 x999'],
  ]

  for (const [delivery_count, redelivery_count, shouldShow, expectedBadge] of cases) {
    const markup = renderToStaticMarkup(renderAttemptInTaskQueue({
      attempt_id: 'att-dedup',
      stage: 'ocr',
      status: 'done',
      delivery_count,
      redelivery_count,
    }))

    if (shouldShow) {
      assert.ok(markup.includes(expectedBadge), `Should display "${expectedBadge}" for delivery=${delivery_count}, redelivery=${redelivery_count}`)
    } else {
      assert.ok(!markup.includes('幂等去重'), `Should NOT display 幂等去重 for delivery=${delivery_count}, redelivery=${redelivery_count}`)
    }
  }
})

test('Adversarial 3: Stage Localization & Formatting Fallback', () => {
  // All known stages in TaskStage
  const knownStages = [
    'queued', 'text', 'face_queued', 'face', 'retry',
    'review', 'error', 'excluded', 'detect', 'ocr', 'feature', 'done'
  ]
  for (const s of knownStages) {
    const markup = renderToStaticMarkup(renderAttemptInTaskQueue({
      attempt_id: 'att-stage',
      stage: s,
      status: 'done',
      delivery_count: 1,
    }))
    const expectedLabel = TASK_STAGE_LABELS[s]
    assert.ok(expectedLabel, `Stage ${s} should have a label in TASK_STAGE_LABELS`)
    assert.ok(markup.includes(expectedLabel), `Markup should include localized label ${expectedLabel} for stage ${s}`)
  }

  // Unknown custom stage
  const unknownMarkup = renderToStaticMarkup(renderAttemptInTaskQueue({
    attempt_id: 'att-stage-custom',
    stage: 'custom_future_stage',
    status: 'done',
    delivery_count: 1,
  }))
  assert.ok(unknownMarkup.includes('custom_future_stage'), 'Unknown stage must fall back to raw stage string safely')
})

test('Adversarial 4: Truth-in-UI Boundary Value Analysis', () => {
  // formatTaskDuration: Never 0s or empty
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')
  assert.equal(formatTaskDuration(0), '未知')
  assert.equal(formatTaskDuration(-500), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')
  assert.equal(formatTaskDuration(Infinity), '未知')
  assert.equal(formatTaskDuration(-Infinity), '未知')
  assert.equal(formatTaskDuration(1), '1 ms')
  assert.equal(formatTaskDuration(999), '999 ms')
  assert.equal(formatTaskDuration(1000), '1.0 秒')
  assert.equal(formatTaskDuration(1500), '1.5 秒')
  assert.equal(formatTaskDuration(1550), '1.6 秒')
  assert.equal(formatTaskDuration(59999), '60.0 秒')
  assert.equal(formatTaskDuration(60000), '1 分 0 秒')
  assert.equal(formatTaskDuration(65432), '1 分 5 秒')
  assert.equal(formatTaskDuration(3665000), '61 分 5 秒')

  // formatLeaseRemaining: Never 0s or empty
  const now = 1000
  assert.equal(formatLeaseRemaining(null, now), '无有效租约')
  assert.equal(formatLeaseRemaining(undefined, now), '无有效租约')
  assert.equal(formatLeaseRemaining(0, now), '无有效租约')
  assert.equal(formatLeaseRemaining(-10, now), '无有效租约')
  assert.equal(formatLeaseRemaining(now - 1, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now + 30, now), '剩 30 秒')
  assert.equal(formatLeaseRemaining(now + 90, now), '剩 1 分 30 秒')

  // getNodeDisplay: Never empty
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(undefined), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getNodeDisplay('   '), '未知')
  assert.equal(getNodeDisplay('node-worker-01'), 'node-worker-01')

  // getAttemptDisplay: Never empty
  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(undefined), '未知')
  assert.equal(getAttemptDisplay(''), '未知')
  assert.equal(getAttemptDisplay('   '), '未知')
  assert.equal(getAttemptDisplay('abc'), 'abc')
  assert.equal(getAttemptDisplay('12345678'), '12345678')
  assert.equal(getAttemptDisplay('1234567890'), '12345678')
})

test('Adversarial 5: Live Next.js Standalone Route Reachability', async () => {
  const standaloneServerPath = path.resolve(ROOT, '.next/standalone/server.js')
  assert.ok(fs.existsSync(standaloneServerPath), 'Standalone server must exist at .next/standalone/server.js')

  const TEST_PORT = 3288
  const env = { ...process.env, PORT: String(TEST_PORT), HOSTNAME: '127.0.0.1' }
  const child = spawn(process.execPath, [standaloneServerPath], { env, stdio: ['ignore', 'pipe', 'pipe'] })

  try {
    // Wait for server ready
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Server start timed out')), 5000)
      child.stdout.on('data', (d) => {
        if (d.toString().includes('Ready in') || d.toString().includes('http://')) {
          clearTimeout(timeout)
          resolve(true)
        }
      })
      child.on('error', (err) => {
        clearTimeout(timeout)
        reject(err)
      })
    })

    // Probe 1: GET /api/processing/task-attempts (Missing vid parameter)
    const resNoVid = await fetch(`http://127.0.0.1:${TEST_PORT}/api/processing/task-attempts`)
    assert.equal(resNoVid.status, 400, 'Must return 400 when vid is omitted')
    assert.equal(resNoVid.headers.get('cache-control'), 'no-store', 'Must return Cache-Control: no-store')
    const bodyNoVid = await resNoVid.json()
    assert.equal(bodyNoVid.ok, false)
    assert.equal(bodyNoVid.error, '缺少 vid 参数')
    assert.deepEqual(bodyNoVid.attempts, [])

    // Probe 2: GET /api/processing/task-attempts?vid= (Empty vid)
    const resEmptyVid = await fetch(`http://127.0.0.1:${TEST_PORT}/api/processing/task-attempts?vid=`)
    assert.equal(resEmptyVid.status, 400, 'Must return 400 when vid is empty string')
    const bodyEmptyVid = await resEmptyVid.json()
    assert.equal(bodyEmptyVid.ok, false)

    // Probe 3: GET /api/processing/task-attempts?vid=test-vid (Valid vid, backend disconnected)
    const resValidVid = await fetch(`http://127.0.0.1:${TEST_PORT}/api/processing/task-attempts?vid=test-vid`)
    assert.equal(resValidVid.headers.get('cache-control'), 'no-store', 'Must return Cache-Control: no-store')
    const bodyValidVid = await resValidVid.json()
    assert.equal(bodyValidVid.ok, false)
    assert.deepEqual(bodyValidVid.attempts, [])
  } finally {
    child.kill('SIGKILL')
  }
})

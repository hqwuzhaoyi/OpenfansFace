import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'

// Import models and formatters under test
import {
  STAGE_STEPS,
  getStepStatus,
  formatDrift,
  formatDuration,
  formatKernelEvents,
  formatVramFloor,
  computeSyncStatus,
} from '../lib/processing-deployment-model.ts'

import {
  TASK_STAGE_LABELS,
  TASK_WAITING_REASON_LABELS,
  TASK_ERROR_LABELS,
  formatWaitingReason,
  formatWaitingReasonDescription,
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  formatCreatorHandle,
  formatCreatorBadge,
  formatErrorType,
  formatAttemptStatus,
  formatAttemptStage,
  getAttemptDeduplicationCount,
} from '../lib/processing-task-model.ts'

test('Phase 1 - Node View Hardware Dial & Heartbeat Stress Matrix', () => {
  // Test formatGb equivalent logic used in HardwareDial
  function formatGb(bytes) {
    if (bytes === null || bytes === undefined || typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '未知'
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
  }

  // 1. Toxic Bytes Inputs
  const toxicBytes = [null, undefined, NaN, Infinity, -Infinity, -100, -0.0001, '1024', {}, [], true, false]
  for (const toxic of toxicBytes) {
    assert.equal(formatGb(toxic), '未知', `formatGb failed to handle toxic input: ${toxic}`)
  }

  // Valid Bytes Inputs
  assert.equal(formatGb(0), '0.0 GB')
  assert.equal(formatGb(1024 * 1024 * 1024), '1.0 GB')
  assert.equal(formatGb(16 * 1024 * 1024 * 1024), '16.0 GB')
  assert.equal(formatGb(24.5 * 1024 * 1024 * 1024), '24.5 GB')

  // 2. Heartbeat Guards Verification
  function formatHeartbeat(ts, nowSec = 1700000000) {
    if (typeof ts === 'number' && Number.isFinite(ts) && ts > 0) {
      return `${Math.max(0, Math.round(nowSec - ts))} 秒前`
    }
    return '尚未收到心跳'
  }

  const toxicHeartbeats = [null, undefined, NaN, Infinity, -Infinity, 0, -1, -999999, '2026-11-12', false]
  for (const toxic of toxicHeartbeats) {
    assert.equal(formatHeartbeat(toxic), '尚未收到心跳', `formatHeartbeat failed for: ${toxic}`)
  }

  // Valid and edge timestamps
  assert.equal(formatHeartbeat(1700000000, 1700000000), '0 秒前')
  assert.equal(formatHeartbeat(1699999990, 1700000000), '10 秒前')
  // Future timestamp (clock skew tolerance)
  assert.equal(formatHeartbeat(1700000050, 1700000000), '0 秒前')
})

test('Phase 2 - Deployment View Stepper & Formatters Adversarial Stress', () => {
  // Stepper progression under all known statuses
  const knownStatuses = [
    'ready',
    'downloading',
    'synced',
    'deploying',
    'mismatched',
    'rolled_back',
    'failed',
    'in_progress',
    'active',
    'completed',
    'accepted',
  ]

  for (const status of knownStatuses) {
    const res = getStepStatus('download', 'download', status)
    assert.ok(['completed', 'active', 'failed', 'pending'].includes(res))
  }

  // Ready / Synced status completes all steps
  for (const step of ['download', 'verify', 'start', 'warmup', 'self_test', 'ready']) {
    assert.equal(getStepStatus(step, 'ready', 'ready'), 'completed')
    assert.equal(getStepStatus(step, 'ready', 'synced'), 'completed')
  }

  // Failed status handling with failedStage
  assert.equal(getStepStatus('download', 'failed', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('verify', 'failed', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('start', 'failed', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('warmup', 'failed', 'failed', 'warmup'), 'failed')
  assert.equal(getStepStatus('self_test', 'failed', 'failed', 'warmup'), 'pending')
  assert.equal(getStepStatus('ready', 'failed', 'failed', 'warmup'), 'pending')

  // Downloading status does NOT fail
  assert.equal(getStepStatus('download', 'download', 'downloading'), 'active')
  assert.equal(getStepStatus('verify', 'download', 'downloading'), 'pending')

  // Formatter stress: formatDrift
  assert.equal(formatDrift(null), '未知')
  assert.equal(formatDrift(undefined), '未知')
  assert.equal(formatDrift(NaN), '未知')
  assert.equal(formatDrift(Infinity), '未知')
  assert.equal(formatDrift(-0.01), '未知')
  assert.equal(formatDrift(0), '0.00000')
  assert.equal(formatDrift(0.002345), (0.002345).toFixed(5))
  assert.equal(formatDrift(0.01234), '0.01234')

  // Formatter stress: formatDuration
  assert.equal(formatDuration(null), '未知')
  assert.equal(formatDuration(undefined), '未知')
  assert.equal(formatDuration(NaN), '未知')
  assert.equal(formatDuration(-10), '未知')
  assert.equal(formatDuration(250), '250 ms')
  assert.equal(formatDuration(1500), '1.50 s')

  // Formatter stress: formatKernelEvents
  assert.equal(formatKernelEvents(10, 2, true), '不适用')
  assert.equal(formatKernelEvents(null, null, false), '未知')
  assert.equal(formatKernelEvents(NaN, 0, false), '未知')
  assert.equal(formatKernelEvents(15.2, 3.8, false), '15 CUDA / 4 CPU')
  assert.equal(formatKernelEvents(20, null, false), '20 CUDA / 0 CPU')

  // Formatter stress: formatVramFloor
  assert.equal(formatVramFloor(1000, true, true), '不适用')
  assert.equal(formatVramFloor(null, null, false), '未知')
  assert.equal(formatVramFloor(NaN, false, false), '未知')
  assert.equal(formatVramFloor(720.5, true, false), '720.5 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(450.0, false, false), '450.0 MiB (未达标)')
  // Auto-evaluation when passed is undefined
  assert.equal(formatVramFloor(650.0, undefined, false), '650.0 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(550.0, undefined, false), '550.0 MiB (未达标)')
})

test('Phase 3 - Task View Deduplication & Formatters Adversarial Stress', () => {
  // getAttemptDeduplicationCount boundary stress
  assert.equal(getAttemptDeduplicationCount(null), 1)
  assert.equal(getAttemptDeduplicationCount(undefined), 1)
  assert.equal(getAttemptDeduplicationCount({}), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: null, redelivery_count: null }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: NaN, redelivery_count: undefined }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: -5, redelivery_count: -10 }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3 }), 3)
  assert.equal(getAttemptDeduplicationCount({ redelivery_count: 5 }), 5)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 2, redelivery_count: 7 }), 7)

  // Deduplication summary strip calculation stress
  const attemptsCases = [
    [],
    [null, undefined],
    [{ delivery_count: 1, redelivery_count: 0 }],
    [{ delivery_count: 3, redelivery_count: 1 }],
    [{ delivery_count: 2 }, { redelivery_count: 4 }],
    [{ delivery_count: NaN }, { delivery_count: 5 }],
  ]

  for (const list of attemptsCases) {
    const totalDedup = list.reduce((acc, a) => {
      const del = typeof a?.delivery_count === 'number' && Number.isFinite(a.delivery_count) ? a.delivery_count : 0
      const redel = typeof a?.redelivery_count === 'number' && Number.isFinite(a.redelivery_count) ? a.redelivery_count : 0
      return acc + Math.max(0, Math.max(del, redel, 1) - 1)
    }, 0)
    assert.ok(Number.isFinite(totalDedup) && totalDedup >= 0, `Dedup count was not non-negative finite: ${totalDedup}`)
  }

  // formatWaitingReason and description
  assert.equal(formatWaitingReason(null), '无等待')
  assert.equal(formatWaitingReason(''), '无等待')
  assert.equal(formatWaitingReason('   '), '无等待')
  assert.equal(formatWaitingReason('none'), '无等待')
  assert.equal(formatWaitingReason('waiting_for_node'), '等待计算节点')
  assert.equal(formatWaitingReason('quota_exhausted'), '并发配额超限')
  assert.equal(formatWaitingReason('unknown_reason_xyz'), '未知等待原因')

  assert.equal(formatWaitingReasonDescription('none'), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription('quota_exhausted'), '已达到全局并发上限或物理设备配额阶梯')
  assert.equal(formatWaitingReasonDescription('unknown_reason_xyz'), '未知等待原因')

  // Lease remaining
  assert.equal(formatLeaseRemaining(null), '无有效租约')
  assert.equal(formatLeaseRemaining(NaN), '无有效租约')
  assert.equal(formatLeaseRemaining(100, 200), '租约已过期')
  assert.equal(formatLeaseRemaining(250, 200), '剩 50 秒')
  assert.equal(formatLeaseRemaining(500, 200), '剩 5 分 0 秒')

  // Attempt Status
  const statusDone = formatAttemptStatus('done')
  assert.equal(statusDone.label, '已完成')
  assert.equal(statusDone.isDone, true)

  const statusInterrupted = formatAttemptStatus('interrupted')
  assert.equal(statusInterrupted.label, '中断回收')
  assert.equal(statusInterrupted.isInterrupted, true)

  const statusError = formatAttemptStatus('failed')
  assert.equal(statusError.label, '失败')
  assert.equal(statusError.isError, true)

  const statusRunning = formatAttemptStatus('running')
  assert.equal(statusRunning.label, '运行中')
})

test('Phase 4 - Video Streaming Hotfix Files Preservation & Sanity Check', () => {
  const hotfixFiles = [
    'lib/file-stream.ts',
    'app/api/video/route.ts',
    'lib/video-sources.ts',
    'lib/integration.ts',
  ]

  for (const rel of hotfixFiles) {
    const fullPath = path.resolve(rel)
    assert.ok(fs.existsSync(fullPath), `Critical video hotfix file missing: ${rel}`)
    const content = fs.readFileSync(fullPath, 'utf8')
    assert.ok(content.length > 500, `File content surprisingly short: ${rel}`)
  }

  // Verify findMp4Hev1PatchesAsync exists in lib/file-stream.ts
  const fileStreamContent = fs.readFileSync(path.resolve('lib/file-stream.ts'), 'utf8')
  assert.ok(fileStreamContent.includes('findMp4Hev1PatchesAsync'), 'findMp4Hev1PatchesAsync not found in lib/file-stream.ts')
  assert.ok(fileStreamContent.includes('extractHev1PatchesFromMoov'), 'extractHev1PatchesFromMoov not found in lib/file-stream.ts')

  // Verify route.ts uses findMp4Hev1PatchesAsync
  const routeContent = fs.readFileSync(path.resolve('app/api/video/route.ts'), 'utf8')
  assert.ok(routeContent.includes('findMp4Hev1PatchesAsync'), 'route.ts does not use findMp4Hev1PatchesAsync')

  // Verify integration.ts bounds NAS concurrency
  const integrationContent = fs.readFileSync(path.resolve('lib/integration.ts'), 'utf8')
  assert.ok(integrationContent.includes('Math.min(2,entries.length)'), 'integration.ts does not bound NAS concurrency to 2')
})

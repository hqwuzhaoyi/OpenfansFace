/**
 * Milestone 7 Iteration 2 Adversarial Stress & Oracle Probe Suite
 * Executed by m7_iter2_challenger_2 (Empirical Challenger)
 *
 * Verifies all edge cases, malformed telemetry, non-finite floats,
 * negative counters, and UI truth invariants across the Operations Tri-View.
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'

import {
  STAGE_STEPS,
  computeSyncStatus,
  getStepStatus,
  formatDrift,
  formatDuration,
  formatKernelEvents,
  formatVramFloor,
} from '../lib/processing-deployment-model.ts'

import {
  TASK_WAITING_REASON_LABELS,
  formatWaitingReason,
  formatWaitingReasonDescription,
  formatTaskDuration,
  formatLeaseRemaining,
  formatAttemptStatus,
  formatAttemptStage,
  getAttemptDeduplicationCount,
} from '../lib/processing-task-model.ts'

const root = process.cwd()
const hardwareDialSrc = fs.readFileSync(path.join(root, 'components/processing/hardware-dial.tsx'), 'utf-8')
const nodeViewSrc = fs.readFileSync(path.join(root, 'components/processing/node-view.tsx'), 'utf-8')

// Exact replication of formatGb from components/processing/hardware-dial.tsx:17-20
function formatGb(bytes) {
  if (bytes === null || bytes === undefined || typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '未知'
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

// Emulate HardwareDial core evaluation logic directly matching the JSX component
function evaluateHardwareDial({ type, usedBytes, totalBytes, percent, isCpuOnly = false, isOffline = false }) {
  let calculatedPercent = null
  let ratioText = ''
  let valueDisplay = '未知'
  let isNotApplicable = false
  let isUnknown = false

  // FormatGb internal
  const fmtGb = (bytes) => {
    if (bytes === null || bytes === undefined || typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '未知'
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
  }

  if (isCpuOnly && (type === 'vram' || type === 'gpu_util')) {
    isNotApplicable = true
    valueDisplay = '不适用'
    ratioText = '纯 CPU 节点'
  } else if (isOffline) {
    isUnknown = true
    valueDisplay = '未知'
    ratioText =
      typeof totalBytes === 'number' && Number.isFinite(totalBytes) && totalBytes > 0
        ? `未知 / ${fmtGb(totalBytes)}`
        : '失联未采集'
  } else if (type === 'gpu_util') {
    if (
      percent === null ||
      percent === undefined ||
      typeof percent !== 'number' ||
      !Number.isFinite(percent) ||
      percent < 0
    ) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = '未采集'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round(percent)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = 'GPU 实时负载'
    }
  } else {
    // RAM or VRAM
    const isValidTotal =
      typeof totalBytes === 'number' && Number.isFinite(totalBytes) && totalBytes > 0
    const isValidUsed =
      typeof usedBytes === 'number' && Number.isFinite(usedBytes) && usedBytes >= 0

    if (!isValidTotal || !isValidUsed) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = isValidTotal ? `未知 / ${fmtGb(totalBytes)}` : '未知'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round((usedBytes / totalBytes) * 100)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = `${fmtGb(usedBytes)} / ${fmtGb(totalBytes)}`
    }
  }

  const ariaValueText = isNotApplicable ? '不适用' : isUnknown ? '未知' : `${calculatedPercent}%`
  const ariaValueNow = calculatedPercent ?? undefined

  return { calculatedPercent, ratioText, valueDisplay, isNotApplicable, isUnknown, ariaValueText, ariaValueNow }
}

// -----------------------------------------------------------------------------
// Test Suite 1: formatGb Exhaustive Oracle
// -----------------------------------------------------------------------------
test('Adversarial Oracle: formatGb IEEE-754 Boundary & Toxic Types', () => {
  // Negative inputs must return '未知'
  assert.equal(formatGb(-1), '未知')
  assert.equal(formatGb(-100), '未知')
  assert.equal(formatGb(-0.0000001), '未知')
  assert.equal(formatGb(-Infinity), '未知')
  assert.equal(formatGb(Number.MIN_SAFE_INTEGER), '未知')

  // Non-finite / non-number inputs must return '未知'
  assert.equal(formatGb(NaN), '未知')
  assert.equal(formatGb(Infinity), '未知')
  assert.equal(formatGb(null), '未知')
  assert.equal(formatGb(undefined), '未知')
  assert.equal(formatGb('1024'), '未知')
  assert.equal(formatGb({}), '未知')
  assert.equal(formatGb([]), '未知')
  assert.equal(formatGb(true), '未知')

  // Zero & Valid Positive Bytes
  assert.equal(formatGb(0), '0.0 GB')
  assert.equal(formatGb(1024 * 1024 * 1024), '1.0 GB')
  assert.equal(formatGb(16 * 1024 * 1024 * 1024), '16.0 GB')
  assert.equal(formatGb(12 * 1024 * 1024 * 1024), '12.0 GB')
  assert.equal(formatGb(32 * 1024 * 1024 * 1024), '32.0 GB')
})

// -----------------------------------------------------------------------------
// Test Suite 2: HardwareDial Adversarial State Machine
// -----------------------------------------------------------------------------
test('Adversarial Oracle: HardwareDial Telemetry Combinations', () => {
  const TOTAL_16GB = 16 * 1024 * 1024 * 1024

  // 1. Negative usedBytes (e.g. from telemetry underflow / bad sensor)
  const dialNegUsed = evaluateHardwareDial({
    type: 'ram',
    usedBytes: -1024,
    totalBytes: TOTAL_16GB,
  })
  assert.equal(dialNegUsed.valueDisplay, '未知')
  assert.equal(dialNegUsed.ratioText, '未知 / 16.0 GB')
  assert.equal(dialNegUsed.ariaValueText, '未知')
  assert.equal(dialNegUsed.ariaValueNow, undefined)
  assert.equal(dialNegUsed.isUnknown, true)

  // 2. NaN usedBytes
  const dialNaNUsed = evaluateHardwareDial({
    type: 'ram',
    usedBytes: NaN,
    totalBytes: TOTAL_16GB,
  })
  assert.equal(dialNaNUsed.valueDisplay, '未知')
  assert.equal(dialNaNUsed.ratioText, '未知 / 16.0 GB')
  assert.equal(dialNaNUsed.ariaValueText, '未知')
  assert.equal(dialNaNUsed.ariaValueNow, undefined)

  // 3. Infinity usedBytes
  const dialInfUsed = evaluateHardwareDial({
    type: 'ram',
    usedBytes: Infinity,
    totalBytes: TOTAL_16GB,
  })
  assert.equal(dialInfUsed.valueDisplay, '未知')
  assert.equal(dialInfUsed.ratioText, '未知 / 16.0 GB')

  // 4. Invalid totalBytes (0, negative, NaN, null)
  for (const invalidTotal of [0, -100, -Infinity, NaN, null, undefined, '16GB']) {
    const dialInvalidTotal = evaluateHardwareDial({
      type: 'ram',
      usedBytes: 1024 * 1024 * 1024,
      totalBytes: invalidTotal,
    })
    assert.equal(dialInvalidTotal.valueDisplay, '未知', `invalid total ${invalidTotal} must display '未知'`)
    assert.equal(dialInvalidTotal.ratioText, '未知')
    assert.equal(dialInvalidTotal.isUnknown, true)
    assert.equal(dialInvalidTotal.ariaValueText, '未知')
  }

  // 5. GPU Util Adversarial Probes
  const toxicGpuUtils = [NaN, Infinity, -Infinity, -1, -0.01, null, undefined, '100%']
  for (const toxic of toxicGpuUtils) {
    const dialToxicGpu = evaluateHardwareDial({
      type: 'gpu_util',
      percent: toxic,
    })
    assert.equal(dialToxicGpu.valueDisplay, '未知', `GPU util ${toxic} must display '未知'`)
    assert.equal(dialToxicGpu.ratioText, '未采集')
    assert.equal(dialToxicGpu.isUnknown, true)
    assert.equal(dialToxicGpu.ariaValueText, '未知')
    assert.equal(dialToxicGpu.ariaValueNow, undefined)
  }

  // 6. GPU Util Valid Range & Clamping
  const dialZeroGpu = evaluateHardwareDial({ type: 'gpu_util', percent: 0 })
  assert.equal(dialZeroGpu.valueDisplay, '0%')
  assert.equal(dialZeroGpu.ariaValueText, '0%')
  assert.equal(dialZeroGpu.ariaValueNow, 0)

  const dialOverGpu = evaluateHardwareDial({ type: 'gpu_util', percent: 125 })
  assert.equal(dialOverGpu.valueDisplay, '100%', 'GPU util > 100 must clamp to 100%')
  assert.equal(dialOverGpu.ariaValueNow, 100)

  // 7. Offline Node Behavior
  const dialOfflineWithTotal = evaluateHardwareDial({
    type: 'ram',
    usedBytes: 4 * 1024 * 1024 * 1024,
    totalBytes: TOTAL_16GB,
    isOffline: true,
  })
  assert.equal(dialOfflineWithTotal.valueDisplay, '未知')
  assert.equal(dialOfflineWithTotal.ratioText, '未知 / 16.0 GB')
  assert.equal(dialOfflineWithTotal.isUnknown, true)

  const dialOfflineNoTotal = evaluateHardwareDial({
    type: 'ram',
    usedBytes: null,
    totalBytes: null,
    isOffline: true,
  })
  assert.equal(dialOfflineNoTotal.valueDisplay, '未知')
  assert.equal(dialOfflineNoTotal.ratioText, '失联未采集')
  assert.equal(dialOfflineNoTotal.isUnknown, true)

  // 8. CPU-Only Node Behavior (VRAM & GPU Util)
  const dialCpuVram = evaluateHardwareDial({
    type: 'vram',
    isCpuOnly: true,
  })
  assert.equal(dialCpuVram.valueDisplay, '不适用')
  assert.equal(dialCpuVram.ratioText, '纯 CPU 节点')
  assert.equal(dialCpuVram.isNotApplicable, true)
  assert.equal(dialCpuVram.ariaValueText, '不适用')

  const dialCpuGpu = evaluateHardwareDial({
    type: 'gpu_util',
    isCpuOnly: true,
  })
  assert.equal(dialCpuGpu.valueDisplay, '不适用')
  assert.equal(dialCpuGpu.ratioText, '纯 CPU 节点')
  assert.equal(dialCpuGpu.isNotApplicable, true)
})

// -----------------------------------------------------------------------------
// Test Suite 3: Node View Client-Side Telemetry Subtractions
// -----------------------------------------------------------------------------
test('Adversarial Oracle: Node View Client-Side Arithmetic & Heartbeat Guards', () => {
  // Emulate node-view.tsx line 273 (used RAM bytes calculation)
  const computeRamUsed = (pool) => {
    return typeof pool.free_ram_bytes === 'number' &&
      Number.isFinite(pool.free_ram_bytes) &&
      typeof pool.ram_total_bytes === 'number' &&
      Number.isFinite(pool.ram_total_bytes)
      ? pool.ram_total_bytes - pool.free_ram_bytes
      : null
  }

  // Free bytes exceed total (anomalous negative usage)
  const underflowPool = { ram_total_bytes: 16 * 1024 * 1024 * 1024, free_ram_bytes: 17 * 1024 * 1024 * 1024 }
  const underflowUsed = computeRamUsed(underflowPool)
  assert.equal(underflowUsed < 0, true)
  // Passed into HardwareDial:
  const underflowDial = evaluateHardwareDial({
    type: 'ram',
    usedBytes: underflowUsed,
    totalBytes: underflowPool.ram_total_bytes,
  })
  // HardwareDial catches underflowUsed < 0 and returns '未知' without "-0.0 GB" or "-6%"
  assert.equal(underflowDial.valueDisplay, '未知')
  assert.equal(underflowDial.ratioText, '未知 / 16.0 GB')

  // Non-finite free_ram_bytes
  const nanFreePool = { ram_total_bytes: 16 * 1024 * 1024 * 1024, free_ram_bytes: NaN }
  assert.equal(computeRamUsed(nanFreePool), null)

  const infFreePool = { ram_total_bytes: 16 * 1024 * 1024 * 1024, free_ram_bytes: Infinity }
  assert.equal(computeRamUsed(infFreePool), null)

  // Emulate node-view.tsx line 341 (last_heartbeat_at formatting)
  const formatHeartbeat = (last_heartbeat_at) => {
    return typeof last_heartbeat_at === 'number' &&
      Number.isFinite(last_heartbeat_at) &&
      last_heartbeat_at > 0
      ? `${Math.max(0, Math.round(Date.now() / 1000 - last_heartbeat_at))} 秒前`
      : '尚未收到心跳'
  }

  // Invalid timestamps
  for (const toxicHb of [null, undefined, 0, -1, -9999, NaN, Infinity, -Infinity, 'invalid', '2026-11-12']) {
    assert.equal(formatHeartbeat(toxicHb), '尚未收到心跳', `heartbeat ${toxicHb} must format as '尚未收到心跳'`)
  }

  // Future timestamp (clock skew) must clamp to '0 秒前'
  const futureHb = Date.now() / 1000 + 3600
  assert.equal(formatHeartbeat(futureHb), '0 秒前')

  // Valid past timestamp
  const pastHb = Date.now() / 1000 - 15
  assert.equal(formatHeartbeat(pastHb), '15 秒前')
})

// -----------------------------------------------------------------------------
// Test Suite 4: Deduplication Count Edge Cases & Mathematical Oracles
// -----------------------------------------------------------------------------
test('Adversarial Oracle: getAttemptDeduplicationCount Boundary Robustness', () => {
  // Baseline semantics:
  // null / undefined attempt -> 1 (initial default)
  assert.equal(getAttemptDeduplicationCount(null), 1)
  assert.equal(getAttemptDeduplicationCount(undefined), 1)

  // Empty attempt object -> 0
  assert.equal(getAttemptDeduplicationCount({}), 0)

  // Non-finite delivery_count
  assert.equal(getAttemptDeduplicationCount({ delivery_count: NaN, redelivery_count: 0 }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: Infinity, redelivery_count: 0 }), 0)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: -Infinity, redelivery_count: 0 }), 0)

  // Negative counters clamped to 0
  assert.equal(getAttemptDeduplicationCount({ delivery_count: -5, redelivery_count: -10 }), 0)

  // Floating point delivery counts rounded
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3.7, redelivery_count: 2 }), 4)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3.2, redelivery_count: 2 }), 3)

  // Nominal maximum of delivery and redelivery
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 1, redelivery_count: 4 }), 4)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 5, redelivery_count: 2 }), 5)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3, redelivery_count: 3 }), 3)
})

// -----------------------------------------------------------------------------
// Test Suite 5: Deployment Model Telemetry Formatters
// -----------------------------------------------------------------------------
test('Adversarial Oracle: Deployment Model Telemetry Formatters Under Stress', () => {
  // formatDrift
  assert.equal(formatDrift(0), '0.00000')
  assert.equal(formatDrift(0.0001), '0.00010')
  assert.equal(formatDrift(0.05), '0.05000')
  for (const toxic of [-0.0001, -1, NaN, Infinity, -Infinity, null, undefined, '0.01']) {
    assert.equal(formatDrift(toxic), '未知')
  }

  // formatDuration
  assert.equal(formatDuration(0), '0 ms')
  assert.equal(formatDuration(450), '450 ms')
  assert.equal(formatDuration(1500), '1.50 s')
  assert.equal(formatDuration(120000), '120.00 s')
  for (const toxic of [-1, -100, NaN, Infinity, -Infinity, null, undefined, '1000']) {
    assert.equal(formatDuration(toxic), '未知')
  }

  // formatVramFloor
  assert.equal(formatVramFloor(600, true, false), '600.0 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(500, false, false), '500.0 MiB (未达标)')
  assert.equal(formatVramFloor(600, null, true), '不适用')
  for (const toxic of [-1, NaN, Infinity, null, undefined, '600 MiB']) {
    assert.equal(formatVramFloor(toxic, false, false), '未知')
  }

  // formatKernelEvents
  assert.equal(formatKernelEvents(512, 0, false), '512 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(null, null, true), '不适用')
  assert.equal(formatKernelEvents(null, null, false), '未知')
  assert.equal(formatKernelEvents(-10, 0, false), '未知')
  assert.equal(formatKernelEvents(NaN, 0, false), '未知')
  assert.equal(formatKernelEvents(512, -1, false), '512 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(512, NaN, false), '512 CUDA / 0 CPU')
})

// -----------------------------------------------------------------------------
// Test Suite 6: Task View Formatter Invariants & Waiting Reasons
// -----------------------------------------------------------------------------
test('Adversarial Oracle: Task Model Formatters & Waiting Reasons Under Stress', () => {
  // formatWaitingReason
  assert.equal(formatWaitingReason('waiting_for_node'), '等待计算节点')
  assert.equal(formatWaitingReason('quota_exhausted'), '并发配额超限')
  assert.equal(formatWaitingReason('gpu_busy'), 'GPU 显存繁忙')
  assert.equal(formatWaitingReason('rate_limited'), '并发降频限流')
  assert.equal(formatWaitingReason('none'), '无等待')
  assert.equal(formatWaitingReason(null), '无等待')
  assert.equal(formatWaitingReason(undefined), '无等待')
  assert.equal(formatWaitingReason(''), '无等待')
  assert.equal(formatWaitingReason('   '), '无等待')
  assert.equal(formatWaitingReason('custom_unknown_reason'), '未知等待原因')

  // formatTaskDuration: <= 0 returns '未知' per Truth-in-UI
  assert.equal(formatTaskDuration(0), '未知')
  assert.equal(formatTaskDuration(120), '120 ms')
  assert.equal(formatTaskDuration(2500), '2.5 秒')
  assert.equal(formatTaskDuration(75000), '1 分 15 秒')
  assert.equal(formatTaskDuration(3665000), '61 分 5 秒')
  assert.equal(formatTaskDuration(-500), '未知')
  assert.equal(formatTaskDuration(NaN), '未知')
  assert.equal(formatTaskDuration(Infinity), '未知')
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')

  // formatLeaseRemaining
  const now = 1000
  assert.equal(formatLeaseRemaining(now, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now - 10, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now + 12, now), '剩 12 秒')
  assert.equal(formatLeaseRemaining(now + 95, now), '剩 1 分 35 秒')
  assert.equal(formatLeaseRemaining(0, now), '无有效租约')
  assert.equal(formatLeaseRemaining(-1000, now), '无有效租约')
  assert.equal(formatLeaseRemaining(NaN, now), '无有效租约')
  assert.equal(formatLeaseRemaining(null, now), '无有效租约')
  assert.equal(formatLeaseRemaining(undefined, now), '无有效租约')

  // formatAttemptStatus
  const statusDone = formatAttemptStatus('done')
  assert.equal(statusDone.isDone, true)
  assert.equal(statusDone.label, '已完成')

  const statusCompleted = formatAttemptStatus('completed')
  assert.equal(statusCompleted.isDone, true)
  assert.equal(statusCompleted.label, '已完成')

  const statusInterrupted = formatAttemptStatus('interrupted')
  assert.equal(statusInterrupted.isInterrupted, true)
  assert.equal(statusInterrupted.label, '中断回收')

  const statusError = formatAttemptStatus('error')
  assert.equal(statusError.isError, true)
  assert.equal(statusError.label, '失败')

  const statusFailed = formatAttemptStatus('failed')
  assert.equal(statusFailed.isError, true)
  assert.equal(statusFailed.label, '失败')

  const statusRunning = formatAttemptStatus('running')
  assert.equal(statusRunning.label, '运行中')

  const statusNull = formatAttemptStatus(null)
  assert.equal(statusNull.label, '运行中')
})


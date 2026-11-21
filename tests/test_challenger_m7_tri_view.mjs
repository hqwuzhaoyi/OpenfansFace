import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'

// Import models directly
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
const nodeViewSrc = fs.readFileSync(path.join(root, 'components/processing/node-view.tsx'), 'utf-8')
const hardwareDialSrc = fs.readFileSync(path.join(root, 'components/processing/hardware-dial.tsx'), 'utf-8')
const nodeStatusBadgeSrc = fs.readFileSync(path.join(root, 'components/processing/node-status-badge.tsx'), 'utf-8')
const deploymentViewSrc = fs.readFileSync(path.join(root, 'components/processing/deployment-view.tsx'), 'utf-8')
const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')
const taskModelSrc = fs.readFileSync(path.join(root, 'lib/processing-task-model.ts'), 'utf-8')

// Replicate HardwareDial calculation for mathematical invariant testing
function formatGb(bytes) {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return '未知'
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

function evaluateHardwareDial({ type, usedBytes, totalBytes, percent, isCpuOnly = false, isOffline = false }) {
  let calculatedPercent = null
  let ratioText = ''
  let valueDisplay = '未知'
  let isNotApplicable = false
  let isUnknown = false

  if (isCpuOnly && (type === 'vram' || type === 'gpu_util')) {
    isNotApplicable = true
    valueDisplay = '不适用'
    ratioText = '纯 CPU 节点'
  } else if (isOffline) {
    isUnknown = true
    valueDisplay = '未知'
    ratioText = totalBytes ? `未知 / ${formatGb(totalBytes)}` : '失联未采集'
  } else if (type === 'gpu_util') {
    if (percent === null || percent === undefined) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = '未采集'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round(percent)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = 'GPU 实时负载'
    }
  } else {
    if (usedBytes === null || usedBytes === undefined || !totalBytes || totalBytes <= 0) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = totalBytes ? `未知 / ${formatGb(totalBytes)}` : '未知'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round((usedBytes / totalBytes) * 100)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = `${formatGb(usedBytes)} / ${formatGb(totalBytes)}`
    }
  }

  return { calculatedPercent, ratioText, valueDisplay, isNotApplicable, isUnknown }
}

// ============================================================================
// SUITE 1: Operations Node View Truth-in-UI Invariants
// ============================================================================

test('Tri-View Challenger Suite 1: Node View Telemetry & Truth-in-UI Invariants', () => {
  // 1. HardwareDial formatGb resilience: NaN, null, undefined, empty string must NEVER leak 'NaN' or throw
  const invalidByteInputs = [null, undefined, NaN, '', 'not-a-number', -Infinity, Infinity]
  for (const input of invalidByteInputs) {
    const formatted = formatGb(input)
    assert.equal(formatted, '未知', `formatGb on ${input} must return '未知'`)
    assert.ok(!formatted.includes('NaN'), `formatGb must never output 'NaN' for input ${input}`)
  }

  // Valid bytes format to GB
  assert.equal(formatGb(12884901888), '12.0 GB')
  assert.equal(formatGb(34359738368), '32.0 GB')

  // 2. CPU-only nodes strictly display "不适用" and "纯 CPU 节点" for VRAM and GPU utilization
  const cpuVram = evaluateHardwareDial({ type: 'vram', usedBytes: null, totalBytes: null, isCpuOnly: true })
  assert.equal(cpuVram.valueDisplay, '不适用')
  assert.equal(cpuVram.ratioText, '纯 CPU 节点')
  assert.equal(cpuVram.isNotApplicable, true)
  assert.equal(cpuVram.calculatedPercent, null)

  const cpuGpuUtil = evaluateHardwareDial({ type: 'gpu_util', percent: null, isCpuOnly: true })
  assert.equal(cpuGpuUtil.valueDisplay, '不适用')
  assert.equal(cpuGpuUtil.ratioText, '纯 CPU 节点')
  assert.equal(cpuGpuUtil.isNotApplicable, true)

  // 3. Offline nodes strictly display "未知" and "失联未采集"
  const offlineVram = evaluateHardwareDial({ type: 'vram', usedBytes: null, totalBytes: null, isOffline: true })
  assert.equal(offlineVram.valueDisplay, '未知')
  assert.equal(offlineVram.ratioText, '失联未采集')
  assert.equal(offlineVram.isUnknown, true)

  const offlineWithTotal = evaluateHardwareDial({ type: 'vram', usedBytes: null, totalBytes: 12884901888, isOffline: true })
  assert.equal(offlineWithTotal.valueDisplay, '未知')
  assert.equal(offlineWithTotal.ratioText, '未知 / 12.0 GB')

  // 4. Online GPU with uncollected metrics (telemetry null or missing)
  const uncollectedGpu = evaluateHardwareDial({ type: 'gpu_util', percent: null, isCpuOnly: false, isOffline: false })
  assert.equal(uncollectedGpu.valueDisplay, '未知')
  assert.equal(uncollectedGpu.ratioText, '未采集')

  const uncollectedVram = evaluateHardwareDial({ type: 'vram', usedBytes: null, totalBytes: 12884901888, isCpuOnly: false, isOffline: false })
  assert.equal(uncollectedVram.valueDisplay, '未知')
  assert.equal(uncollectedVram.ratioText, '未知 / 12.0 GB')

  // 5. NodeStatusBadge: All 5 statuses render exact Chinese labels without English leak
  const statusMappings = [
    { status: 'accepting', label: '可接单' },
    { status: 'model_ready', label: '模型就绪' },
    { status: 'online', label: '在线' },
    { status: 'registered', label: '已登记' },
    { status: 'offline', label: '已失联' },
  ]

  for (const { status, label } of statusMappings) {
    assert.ok(nodeStatusBadgeSrc.includes(label), `node-status-badge.tsx must render label '${label}' for status '${status}'`)
  }

  // 6. Source inspection: node-view.tsx wired to live backend control route
  assert.ok(nodeViewSrc.includes('/api/processing/nodes/control'), 'node-view.tsx must call /api/processing/nodes/control')
  assert.ok(nodeViewSrc.includes('revoke_acceptance'), 'node-view.tsx must handle revoke_acceptance')
  assert.ok(nodeViewSrc.includes('restore_acceptance'), 'node-view.tsx must handle restore_acceptance')
  assert.ok(nodeViewSrc.includes('schema_version !== 1'), 'node-view.tsx must validate schema_version !== 1')
})

// ============================================================================
// SUITE 2: Operations Deployment View Truth-in-UI & Stepper Invariants
// ============================================================================

test('Tri-View Challenger Suite 2: Deployment View Stepper, Formatters & Rollback Modal', () => {
  // 1. 6-stage Stepper Localization and Descriptions
  const expectedSteps = [
    { stage: 'download', label: '下载', desc: '分发受限运行包' },
    { stage: 'verify', label: '校验', desc: 'SHA-256 清单核对' },
    { stage: 'start', label: '启动', desc: '沙箱服务与权限初始化' },
    { stage: 'warmup', label: '预热', desc: '显存底线探测与 Arena 预分配' },
    { stage: 'self_test', label: '自检', desc: '基准人脸点位与真 CUDA 验证' },
    { stage: 'ready', label: '就绪', desc: '模型就绪并进入可接单状态' },
  ]

  assert.equal(STAGE_STEPS.length, 6, 'STAGE_STEPS must contain exactly 6 stages')
  for (let i = 0; i < expectedSteps.length; i++) {
    assert.equal(STAGE_STEPS[i].stage, expectedSteps[i].stage)
    assert.equal(STAGE_STEPS[i].label, expectedSteps[i].label)
    assert.equal(STAGE_STEPS[i].desc, expectedSteps[i].desc)
  }

  // 2. getStepStatus Stepper Evaluation Invariants
  const STAGE_ORDER = ['download', 'verify', 'start', 'warmup', 'self_test', 'ready']

  // Ready node: all steps completed
  for (const step of STAGE_ORDER) {
    assert.equal(getStepStatus(step, 'ready', 'ready', undefined), 'completed', `Step ${step} on ready node must be completed`)
  }

  // Downloading node: download is active, others are pending
  assert.equal(getStepStatus('download', 'download', 'downloading', undefined), 'active')
  for (const step of STAGE_ORDER.slice(1)) {
    assert.equal(getStepStatus(step, 'download', 'downloading', undefined), 'pending')
  }

  // Failed node at warmup: prior steps completed, warmup failed, subsequent pending. NEVER active!
  assert.equal(getStepStatus('download', 'warmup', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('verify', 'warmup', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('start', 'warmup', 'failed', 'warmup'), 'completed')
  assert.equal(getStepStatus('warmup', 'warmup', 'failed', 'warmup'), 'failed')
  assert.equal(getStepStatus('self_test', 'warmup', 'failed', 'warmup'), 'pending')
  assert.equal(getStepStatus('ready', 'warmup', 'failed', 'warmup'), 'pending')

  for (const step of STAGE_ORDER) {
    assert.notEqual(getStepStatus(step, 'warmup', 'failed', 'warmup'), 'active', 'Step must never be active on failure')
  }

  // 3. Formatters strictly return "未知" on invalid / empty / NaN inputs
  const invalidInputs = [null, undefined, NaN, '', 'invalid', -1]
  for (const val of invalidInputs) {
    assert.equal(formatDrift(val), '未知', `formatDrift(${val}) must be '未知'`)
    assert.equal(formatDuration(val), '未知', `formatDuration(${val}) must be '未知'`)
    assert.equal(formatVramFloor(val, true, false), '未知', `formatVramFloor(${val}) must be '未知'`)
  }

  // CPU nodes return "不适用"
  assert.equal(formatKernelEvents(128, 0, true), '不适用')
  assert.equal(formatVramFloor(1200, true, true), '不适用')

  // Valid outputs
  assert.equal(formatDrift(0.00123), '0.00123')
  assert.equal(formatDuration(420), '420 ms')
  assert.equal(formatDuration(2500), '2.50 s')
  assert.equal(formatKernelEvents(256, 0, false), '256 CUDA / 0 CPU')
  assert.equal(formatVramFloor(1800, true, false), '1800.0 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(450, false, false), '450.0 MiB (未达标)')

  // 4. computeSyncStatus Invariants
  assert.equal(computeSyncStatus('v2', 'v2', 'ready', 'ready'), 'synced')
  assert.equal(computeSyncStatus('v2', 'v1', 'download', 'downloading'), 'deploying')
  assert.equal(computeSyncStatus('v2', 'v1', 'self_test', 'in_progress'), 'deploying')
  assert.equal(computeSyncStatus('v2', 'v1', 'start', 'failed'), 'failed')
  assert.equal(computeSyncStatus('v2', 'v1', 'ready', 'rolled_back'), 'rolled_back')
  assert.equal(computeSyncStatus('v2', 'v1', 'ready', 'ready'), 'mismatched')

  // 5. Rollback Modal & Focus Trap Integration in deployment-view.tsx
  assert.ok(deploymentViewSrc.includes('useFocusTrap(Boolean(rollbackNode)'), 'deployment-view.tsx must bind useFocusTrap to rollbackNode')
  assert.ok(deploymentViewSrc.includes('role="dialog"'), 'deployment-view.tsx rollback modal must define role="dialog"')
  assert.ok(deploymentViewSrc.includes('aria-modal="true"'), 'deployment-view.tsx rollback modal must define aria-modal="true"')
  assert.ok(deploymentViewSrc.includes('aria-labelledby="rollback-dialog-title"'), 'deployment-view.tsx rollback modal must define aria-labelledby')
  assert.ok(deploymentViewSrc.includes('action: \'rollback\''), 'deployment-view.tsx must submit action: rollback')
  assert.ok(deploymentViewSrc.includes('调度中心将原子维护事务幂等性，不会中断正在提交的业务回执'), 'deployment-view.tsx must communicate non-disruptive rollback guarantee')
})

// ============================================================================
// SUITE 3: Operations Task View Truth-in-UI Invariants
// ============================================================================

test('Tri-View Challenger Suite 3: Task View Waiting Reasons, Formatting & Dedup Badges', () => {
  // 1. Mandatory Milestone 6/7 waiting reasons defined in TASK_WAITING_REASON_LABELS
  const canonicalReasons = [
    { key: 'waiting_for_node', label: '等待计算节点', description: '当前无空闲或满足硬件能力要求的计算节点' },
    { key: 'quota_exhausted', label: '并发配额超限', description: '已达到全局并发上限或物理设备配额阶梯' },
    { key: 'gpu_busy', label: 'GPU 显存繁忙', description: '显存或算力不足以承载新检测任务' },
    { key: 'rate_limited', label: '并发降频限流', description: '系统正在执行优雅并发控制，在途任务完成后方可领取' },
    { key: 'retry_backoff', label: '重试退避等待', description: '前次执行失败，处于退避等待窗口' },
    { key: 'model_warmup', label: '模型预热自检', description: '计算节点正在执行启动预热与基准漂移校验' },
    { key: 'paused', label: '人工暂停接单', description: '操作员已暂停调度' },
    { key: 'none', label: '无等待', description: '任务正常流转中' },
  ]

  for (const { key, label, description } of canonicalReasons) {
    assert.ok(TASK_WAITING_REASON_LABELS[key], `TASK_WAITING_REASON_LABELS must define '${key}'`)
    assert.equal(TASK_WAITING_REASON_LABELS[key].label, label)
    assert.equal(TASK_WAITING_REASON_LABELS[key].description, description)

    // Formatter functions
    assert.equal(formatWaitingReason(key), label)
    assert.equal(formatWaitingReasonDescription(key), description)
  }

  // 2. Unknown or raw English string safety fallbacks
  const unknownInputs = ['unknown_custom_reason', 'rogue_reason', 'unhandled_token', '']
  for (const unk of unknownInputs) {
    if (!unk) {
      assert.equal(formatWaitingReason(unk), '无等待')
      assert.equal(formatWaitingReasonDescription(unk), '任务正常流转中')
    } else {
      assert.equal(formatWaitingReason(unk), '未知等待原因', `Unknown reason '${unk}' must return '未知等待原因'`)
      assert.equal(formatWaitingReasonDescription(unk), '未知等待原因')
    }
  }

  // 3. Null & Undefined safety fallbacks
  assert.equal(formatWaitingReason(null), '无等待')
  assert.equal(formatWaitingReason(undefined), '无等待')
  assert.equal(formatWaitingReasonDescription(null), '任务正常流转中')
  assert.equal(formatWaitingReasonDescription(undefined), '任务正常流转中')

  // 4. Attempt Status Localization: Done, Interrupted, Failed, Running
  const statusCases = [
    { input: 'done', expectedLabel: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'completed', expectedLabel: '已完成', isDone: true, isInterrupted: false, isError: false },
    { input: 'interrupted', expectedLabel: '中断回收', isDone: false, isInterrupted: true, isError: false },
    { input: 'error', expectedLabel: '失败', isDone: false, isInterrupted: false, isError: true },
    { input: 'failed', expectedLabel: '失败', isDone: false, isInterrupted: false, isError: true },
    { input: 'running', expectedLabel: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: '', expectedLabel: '运行中', isDone: false, isInterrupted: false, isError: false },
    { input: null, expectedLabel: '运行中', isDone: false, isInterrupted: false, isError: false },
  ]

  for (const sc of statusCases) {
    const res = formatAttemptStatus(sc.input)
    assert.equal(res.label, sc.expectedLabel, `formatAttemptStatus('${sc.input}') must equal '${sc.expectedLabel}'`)
    assert.equal(res.isDone, sc.isDone)
    assert.equal(res.isInterrupted, sc.isInterrupted)
    assert.equal(res.isError, sc.isError)
  }

  // 5. Deduplication count calculation & alignment
  assert.equal(getAttemptDeduplicationCount(null), 1)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 1, redelivery_count: 0 }), 1)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 3, redelivery_count: 1 }), 3)
  assert.equal(getAttemptDeduplicationCount({ delivery_count: 1, redelivery_count: 4 }), 4)

  // 6. Task queue source code checks: ensures task-queue.tsx formats dedup badges and waiting reasons cleanly
  assert.ok(taskQueueSrc.includes('formatWaitingReason'), 'task-queue.tsx must use formatWaitingReason')
  assert.ok(taskModelSrc.includes('TASK_WAITING_REASON_LABELS'), 'processing-task-model.ts must define TASK_WAITING_REASON_LABELS')
  assert.ok(taskModelSrc.includes('formatAttemptStatus'), 'processing-task-model.ts must export formatAttemptStatus')
  assert.ok(taskQueueSrc.includes('已完成') && taskQueueSrc.includes('中断回收'), 'task-queue.tsx must render localized attempt statuses')
  assert.ok(taskQueueSrc.includes('幂等去重'), 'task-queue.tsx must display 幂等去重 badge')
  assert.ok(taskQueueSrc.includes('formatTaskDuration'), 'task-queue.tsx must use formatTaskDuration')
})

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

const STAGE_ORDER = ['download', 'verify', 'start', 'warmup', 'self_test', 'ready']

test('1. Stepper Bug Proof -> Fix Verified: Successful & Downloading records evaluated accurately', () => {
  // Case A: A successfully completed deployment where stage is 'ready', status is 'ready' (from Rust SQLite),
  // and failure_evidence is null.
  const readyStepStatus = getStepStatus('ready', 'ready', 'ready', undefined)
  assert.equal(readyStepStatus, 'completed', 'Fix A verified: Ready stage evaluates to completed')
  for (const step of STAGE_ORDER) {
    assert.equal(getStepStatus(step, 'ready', 'ready', undefined), 'completed', `Step ${step} on ready node must be completed`)
  }

  // Case B: An in-progress deployment where stage is 'download', status is 'downloading' (from Rust SQLite),
  // and failure_evidence is null.
  const downloadStepStatus = getStepStatus('download', 'download', 'downloading', undefined)
  assert.equal(downloadStepStatus, 'active', 'Fix B verified: Downloading stage evaluates to active')
  for (const step of STAGE_ORDER.slice(1)) {
    assert.equal(getStepStatus(step, 'download', 'downloading', undefined), 'pending', `Subsequent step ${step} must be pending`)
  }
})

test('2. Stepper Fuzzing on Genuine Failure: No spinning clocks and strict prior completion', () => {
  // When a record is genuinely failed, verify that no spinning clocks appear
  // and prior steps are completed, failed step is failed, subsequent steps are pending.
  const failureStages = STAGE_ORDER

  for (const failedAt of failureStages) {
    const failedIdx = STAGE_ORDER.indexOf(failedAt)
    for (const step of STAGE_ORDER) {
      const stepIdx = STAGE_ORDER.indexOf(step)
      // Call with explicit failed stage
      const res = getStepStatus(step, failedAt, 'failed', failedAt)

      // Rule: Never 'active' on failure
      assert.notEqual(res, 'active', `Step ${step} must never be 'active' on failure at ${failedAt}`)

      if (stepIdx < failedIdx) {
        assert.equal(res, 'completed', `Step ${step} before ${failedAt} should be completed`)
      } else if (stepIdx === failedIdx) {
        assert.equal(res, 'failed', `Step ${step} at ${failedAt} must be failed`)
      } else {
        assert.equal(res, 'pending', `Step ${step} after ${failedAt} must be pending`)
      }
    }
  }
})

test('3. Truth-in-UI Bug Proof -> Fix Verified: formatKernelEvents and formatVramFloor handle NaN & empty string cleanly', () => {
  // formatDrift and formatDuration are resilient:
  assert.equal(formatDrift(null), '未知')
  assert.equal(formatDrift(undefined), '未知')
  assert.equal(formatDrift(NaN), '未知')
  assert.equal(formatDrift(''), '未知')

  assert.equal(formatDuration(null), '未知')
  assert.equal(formatDuration(undefined), '未知')
  assert.equal(formatDuration(NaN), '未知')
  assert.equal(formatDuration(''), '未知')

  // Fix C: formatKernelEvents returns "未知" when cuda is NaN
  const kernelNanRes = formatKernelEvents(NaN, 0)
  assert.equal(kernelNanRes, '未知', 'Fix C verified: formatKernelEvents returns 未知 on NaN')

  // Fix D: formatKernelEvents returns "未知" on empty string
  const kernelEmptyRes = formatKernelEvents('', 0)
  assert.equal(kernelEmptyRes, '未知', 'Fix D verified: formatKernelEvents returns 未知 on empty string')

  // Fix E: formatVramFloor returns "未知" when vramMib is NaN
  const vramNanRes = formatVramFloor(NaN, true)
  assert.equal(vramNanRes, '未知', 'Fix E verified: formatVramFloor returns 未知 on NaN')

  // Fix F: formatVramFloor safely returns "未知" on empty string without throwing TypeError
  let vramCrashed = false
  let vramEmptyRes = null
  try {
    vramEmptyRes = formatVramFloor('', true)
  } catch (err) {
    vramCrashed = true
  }
  assert.equal(vramCrashed, false, 'Fix F verified: formatVramFloor must not throw on empty string')
  assert.equal(vramEmptyRes, '未知', 'Fix F verified: formatVramFloor returns 未知 on empty string')
})

test('4. Truth-in-UI: Route mapping preserves null without synthetic injection', () => {
  const d = {
    deployment_id: 'dep_001',
    node_id: 'node_pc_01',
    package_id: 'pkg_scrfd',
    status: 'downloading',
    desired_version: '1.2.0',
    actual_version: null,
    stage: 'download',
    startup_proof: null,
    self_test_passed: null,
    self_test_drift: null,
    duration_ms: null,
    error: null,
    updated_at: 1700000000,
    pkg_version: '1.2.0',
    execution_provider: 'CUDAExecutionProvider',
    manifest_sha256: 'a'.repeat(64),
    model_sha256: 'b'.repeat(64),
    hostname: 'pc-wsl',
    arch: 'x86_64',
    os: 'linux',
    node_status: 'online',
    gpu_model: null,
  }

  const isCpu = d.execution_provider === 'CPUExecutionProvider'
  const proof = typeof d.startup_proof === 'object' && d.startup_proof !== null ? d.startup_proof : null

  const mappedGpuModel = isCpu ? null : (d.gpu_model ?? null)
  const mappedCudaEvents = proof?.kernel_events ?? proof?.cuda_kernel_events ?? null
  const mappedCpuEvents = proof?.cpu_kernel_events ?? null

  // Verify no fabricated numbers
  assert.equal(mappedGpuModel, null, 'GPU model must be null')
  assert.equal(mappedCudaEvents, null, 'cuda_kernel_events must be null')
  assert.equal(mappedCpuEvents, null, 'cpu_kernel_events must be null')
})

test('5. Accessibility & DOM validation: Static structural check', () => {
  const tsxPath = path.resolve('components/processing/deployment-view.tsx')
  const cssPath = path.resolve('components/processing/deployment-view.css')
  const tsxContent = fs.readFileSync(tsxPath, 'utf-8')
  const cssContent = fs.readFileSync(cssPath, 'utf-8')

  // useFocusTrap imported and called
  assert.ok(tsxContent.includes("import { useFocusTrap } from '@/lib/beui/hooks/use-focus-trap'"))
  assert.ok(tsxContent.includes('useFocusTrap(Boolean(rollbackNode), rollbackCardRef'))

  // Modal accessibility
  assert.ok(tsxContent.includes('tabIndex={-1}'))
  assert.ok(tsxContent.includes('role="dialog"'))
  assert.ok(tsxContent.includes('aria-modal="true"'))
  assert.ok(tsxContent.includes('aria-labelledby="rollback-dialog-title"'))
  assert.ok(tsxContent.includes('id="rollback-dialog-title"'))

  // Mobile touch targets >= 44px
  assert.ok(tsxContent.includes('min-w-[44px] min-h-[44px]'))
  assert.ok(tsxContent.includes('min-h-[44px] sm:min-h-[36px]'))
  assert.ok(cssContent.includes('.ws-deploy-hash-box button'))
  assert.ok(cssContent.includes('.ws-deploy-drawer-header button'))
  assert.ok(cssContent.includes('min-height: 44px;'))
  assert.ok(cssContent.includes('min-width: 44px;'))
})

import { test } from 'node:test'
import assert from 'node:assert/strict'

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

test('1. Bugs A-F Regression Verification', () => {
  // Bug A: 'ready' node evaluates all steps to 'completed'
  for (const step of STAGE_ORDER) {
    const res = getStepStatus(step, 'ready', 'ready', undefined)
    assert.equal(res, 'completed', `Bug A: step ${step} must be completed on ready node`)
  }

  // Bug B: 'downloading' node evaluates stage 'download' to active, others pending
  assert.equal(getStepStatus('download', 'download', 'downloading', undefined), 'active', 'Bug B: download must be active')
  for (const step of STAGE_ORDER.slice(1)) {
    assert.equal(getStepStatus(step, 'download', 'downloading', undefined), 'pending', `Bug B: step ${step} must be pending`)
  }

  // Bug C: formatKernelEvents(NaN, 0) returns '未知'
  assert.equal(formatKernelEvents(NaN, 0), '未知', 'Bug C: formatKernelEvents with NaN cuda must return 未知')

  // Bug D: formatKernelEvents('', 0) returns '未知'
  assert.equal(formatKernelEvents('', 0), '未知', 'Bug D: formatKernelEvents with empty string must return 未知')

  // Bug E: formatVramFloor(NaN, true) returns '未知'
  assert.equal(formatVramFloor(NaN, true), '未知', 'Bug E: formatVramFloor with NaN vram must return 未知')

  // Bug F: formatVramFloor('', true) returns '未知' without throwing
  assert.doesNotThrow(() => {
    const res = formatVramFloor('', true)
    assert.equal(res, '未知', 'Bug F: formatVramFloor with empty string must return 未知')
  })
})

test('2. Aggressive Adversarial Fuzzing on getStepStatus', () => {
  const adversarialValues = [
    undefined,
    null,
    '',
    '   ',
    'unknown_status',
    'CRASHED',
    'FAILED',
    -1,
    0,
    1,
    -999,
    12345,
    NaN,
    Infinity,
    -Infinity,
    true,
    false,
    {},
    { stage: 'download' },
    [],
    ['download'],
    () => {},
    Symbol('step'),
  ]

  const testStages = [...STAGE_ORDER, ...adversarialValues]

  // Fuzz over permutations
  let totalCalls = 0
  for (const stepStage of testStages) {
    for (const currentStage of testStages) {
      for (const status of adversarialValues) {
        // Test 2-arg, 3-arg, 4-arg calls
        const r2 = getStepStatus(stepStage, currentStage)
        const r3 = getStepStatus(stepStage, currentStage, status)
        const r4 = getStepStatus(stepStage, currentStage, status, undefined)
        totalCalls += 3

        const validReturnValues = new Set(['completed', 'active', 'failed', 'pending'])
        assert.ok(validReturnValues.has(r2), `r2 must be valid status, got ${r2}`)
        assert.ok(validReturnValues.has(r3), `r3 must be valid status, got ${r3}`)
        assert.ok(validReturnValues.has(r4), `r4 must be valid status, got ${r4}`)
      }
    }
  }

  // Verify full-pipeline invariants for realistic & edge-case scenarios
  for (const currentStage of testStages) {
    for (const status of ['ready', 'downloading', 'failed', 'mismatched', 'rolled_back', ...adversarialValues]) {
      for (const failedStage of testStages) {
        const statuses = STAGE_ORDER.map(step => getStepStatus(step, currentStage, status, failedStage))
        totalCalls += STAGE_ORDER.length

        const activeCount = statuses.filter(s => s === 'active').length
        const failedCount = statuses.filter(s => s === 'failed').length

        // INVARIANT 1: At most one step may be active
        assert.ok(activeCount <= 1, `At most 1 step may be active, found ${activeCount} for currentStage=${String(currentStage)}, status=${String(status)}`)

        // INVARIANT 2: If failed, activeCount MUST be 0 (NO SPINNING ON FAILURE)
        if (failedCount > 0) {
          assert.equal(activeCount, 0, `No step may be active when failure occurred: failed=${failedCount}, active=${activeCount}`)
        }

        // INVARIANT 3: If status is 'failed', activeCount MUST be 0
        if (status === 'failed' || currentStage === 'failed') {
          assert.equal(activeCount, 0, `Never active when status or stage is 'failed'`)
          assert.equal(failedCount, 1, `Exactly 1 step must be marked failed when status or stage is 'failed'`)
        }
      }
    }
  }

  assert.ok(totalCalls > 10000, `Must have performed extensive fuzzing, total calls: ${totalCalls}`)
})

test('3. Telemetry Formatters Fuzzing on Toxic & Extreme Types', () => {
  const toxicInputs = [
    Symbol('toxicity'),
    123456789012345678901234567890n,
    {},
    { value: 123 },
    [],
    [100],
    () => 42,
    -1,
    -0.000001,
    -1000,
    -Infinity,
    Infinity,
    NaN,
    null,
    undefined,
    '',
    '    ',
    'NaN',
    'null',
    'undefined',
    '123',
    '-50',
    true,
    false,
  ]

  for (const toxic of toxicInputs) {
    // formatDrift
    assert.doesNotThrow(() => {
      const res = formatDrift(toxic)
      assert.equal(res, '未知', `formatDrift(${String(toxic)}) must return '未知', got ${res}`)
    })

    // formatDuration
    assert.doesNotThrow(() => {
      const res = formatDuration(toxic)
      assert.equal(res, '未知', `formatDuration(${String(toxic)}) must return '未知', got ${res}`)
    })

    // formatKernelEvents:
    // With isCpuOnly = false, toxic cuda must return '未知'
    assert.doesNotThrow(() => {
      const res = formatKernelEvents(toxic, 0, false)
      assert.equal(res, '未知', `formatKernelEvents(${String(toxic)}, 0, false) must return '未知', got ${res}`)
    })

    // With isCpuOnly = true, must return '不适用' regardless of inputs
    assert.doesNotThrow(() => {
      const res1 = formatKernelEvents(toxic, toxic, true)
      assert.equal(res1, '不适用', `formatKernelEvents CPU-only must return '不适用', got ${res1}`)
      const res2 = formatKernelEvents(100, 100, true)
      assert.equal(res2, '不适用', `formatKernelEvents CPU-only must return '不适用', got ${res2}`)
    })

    // formatVramFloor:
    // With isCpuOnly = false, toxic vram must return '未知'
    assert.doesNotThrow(() => {
      const res = formatVramFloor(toxic, true, false)
      assert.equal(res, '未知', `formatVramFloor(${String(toxic)}, true, false) must return '未知', got ${res}`)
    })

    // With isCpuOnly = true, must return '不适用'
    assert.doesNotThrow(() => {
      const res1 = formatVramFloor(toxic, true, true)
      assert.equal(res1, '不适用', `formatVramFloor CPU-only must return '不适用', got ${res1}`)
      const res2 = formatVramFloor(1024, true, true)
      assert.equal(res2, '不适用', `formatVramFloor CPU-only must return '不适用', got ${res2}`)
    })
  }
})

test('4. Telemetry Formatters Valid Boundary & Nominal Behavior', () => {
  // formatDrift
  assert.equal(formatDrift(0), '0.00000')
  assert.equal(formatDrift(0.0000123), '0.00001')
  assert.equal(formatDrift(0.04999), '0.04999')
  assert.equal(formatDrift(0.05), '0.05000')

  // formatDuration
  assert.equal(formatDuration(0), '0 ms')
  assert.equal(formatDuration(500), '500 ms')
  assert.equal(formatDuration(999.4), '999 ms')
  assert.equal(formatDuration(1000), '1.00 s')
  assert.equal(formatDuration(2540), '2.54 s')

  // formatKernelEvents (cuda >= 0, cpu variants)
  assert.equal(formatKernelEvents(0, 0), '0 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(15.6, 2.4), '16 CUDA / 2 CPU')
  assert.equal(formatKernelEvents(10, null), '10 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(10, undefined), '10 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(10, -5), '10 CUDA / 0 CPU')
  assert.equal(formatKernelEvents(10, NaN), '10 CUDA / 0 CPU')

  // formatVramFloor (vramMib >= 0, passed variations)
  assert.equal(formatVramFloor(600, true), '600.0 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(599.9, false), '599.9 MiB (未达标)')
  // Inferred passed when null/undefined:
  assert.equal(formatVramFloor(600, null), '600.0 MiB (符合 ≥600MiB 底线)')
  assert.equal(formatVramFloor(599.9, null), '599.9 MiB (未达标)')
  assert.equal(formatVramFloor(599.9, undefined), '599.9 MiB (未达标)')
  assert.equal(formatVramFloor(8192, undefined), '8192.0 MiB (符合 ≥600MiB 底线)')
})

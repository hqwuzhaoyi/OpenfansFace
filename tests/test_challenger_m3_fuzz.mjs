import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  TASK_STAGE_LABELS,
} from '../lib/processing-task-model.ts'

const ADVERSARIAL_INPUTS = [
  undefined,
  null,
  NaN,
  Infinity,
  -Infinity,
  0,
  -0,
  -1,
  -9999999,
  '',
  '   ',
  '\t\n\r',
  'invalid_string',
  '0',
  '-100',
  '1000',
  true,
  false,
  Symbol('fuzz_symbol'),
  0n,
  100n,
  -50n,
  {},
  { value: 123 },
  [],
  [1, 2],
  () => {},
]

test('1. Fuzz formatTaskDuration: zero crashes & strict adherence to 未知 for invalid', () => {
  for (const input of ADVERSARIAL_INPUTS) {
    let result
    let threw = false
    let error
    try {
      result = formatTaskDuration(input)
    } catch (e) {
      threw = true
      error = e
    }

    assert.equal(
      threw,
      false,
      `formatTaskDuration threw an error for input ${String(input)}: ${error?.message}`
    )

    // Check if input is a valid positive finite number
    const isPositiveNumber = typeof input === 'number' && Number.isFinite(input) && input > 0
    if (!isPositiveNumber) {
      assert.equal(
        result,
        '未知',
        `formatTaskDuration(${String(input)}) should return "未知", but got "${result}"`
      )
    } else {
      assert.ok(typeof result === 'string' && result.length > 0)
    }
  }

  // Positive number formatting checks
  assert.equal(formatTaskDuration(500), '500 ms')
  assert.equal(formatTaskDuration(1500), '1.5 秒')
  assert.equal(formatTaskDuration(65000), '1 分 5 秒')
  assert.equal(formatTaskDuration(125400), '2 分 5 秒')
})

test('2. Fuzz getNodeDisplay: zero crashes & strict adherence to 未知 for invalid', () => {
  for (const input of ADVERSARIAL_INPUTS) {
    let result
    let threw = false
    let error
    try {
      result = getNodeDisplay(input)
    } catch (e) {
      threw = true
      error = e
    }

    assert.equal(
      threw,
      false,
      `getNodeDisplay threw for input ${String(input)}: ${error?.message}`
    )

    const isValidNonEmptyString = typeof input === 'string' && input.trim().length > 0
    if (!isValidNonEmptyString) {
      assert.equal(
        result,
        '未知',
        `getNodeDisplay(${String(input)}) should return "未知", but got "${result}"`
      )
    } else {
      assert.equal(result, input.trim())
    }
  }

  assert.equal(getNodeDisplay('  node-wsl-4070ti  '), 'node-wsl-4070ti')
  assert.equal(getNodeDisplay('node-mac-m2'), 'node-mac-m2')
})

test('3. Fuzz getAttemptDisplay: zero crashes & strict adherence to 未知 for invalid', () => {
  for (const input of ADVERSARIAL_INPUTS) {
    let result
    let threw = false
    let error
    try {
      result = getAttemptDisplay(input)
    } catch (e) {
      threw = true
      error = e
    }

    assert.equal(
      threw,
      false,
      `getAttemptDisplay threw for input ${String(input)}: ${error?.message}`
    )

    const isValidNonEmptyString = typeof input === 'string' && input.trim().length > 0
    if (!isValidNonEmptyString) {
      assert.equal(
        result,
        '未知',
        `getAttemptDisplay(${String(input)}) should return "未知", but got "${result}"`
      )
    } else {
      const trimmed = input.trim()
      const expected = trimmed.length > 8 ? trimmed.slice(0, 8) : trimmed
      assert.equal(result, expected)
    }
  }

  assert.equal(getAttemptDisplay('run_abc1234567'), 'run_abc1')
  assert.equal(getAttemptDisplay('short'), 'short')
  assert.equal(getAttemptDisplay('  run_padded  '), 'run_padd')
})

test('4. Fuzz formatLeaseRemaining: zero crashes & lease states', () => {
  // Test leaseExpiresAt with adversarial values
  for (const input of ADVERSARIAL_INPUTS) {
    let result
    let threw = false
    let error
    try {
      result = formatLeaseRemaining(input, 1700000000)
    } catch (e) {
      threw = true
      error = e
    }

    assert.equal(
      threw,
      false,
      `formatLeaseRemaining threw for leaseExpiresAt=${String(input)}: ${error?.message}`
    )

    const isPositiveNumber = typeof input === 'number' && Number.isFinite(input) && input > 0
    if (!isPositiveNumber) {
      assert.equal(
        result,
        '无有效租约',
        `formatLeaseRemaining(${String(input)}) should return "无有效租约", but got "${result}"`
      )
    }
  }

  const now = 1700000000
  // Test valid lease values against various now values
  assert.equal(formatLeaseRemaining(now - 10, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now + 30, now), '剩 30 秒')
  assert.equal(formatLeaseRemaining(now + 90, now), '剩 1 分 30 秒')
  assert.equal(formatLeaseRemaining(now + 125, now), '剩 2 分 5 秒')

  // What if nowSec is undefined? (should use Date.now() / 1000)
  assert.doesNotThrow(() => {
    const res = formatLeaseRemaining(Date.now() / 1000 + 60)
    assert.match(res, /^剩 /)
  })

  // Adversarial nowSec values when leaseExpiresAt is a valid future timestamp
  const futureLease = 1700000100
  for (const nowInput of ADVERSARIAL_INPUTS) {
    let result
    let threw = false
    let error
    try {
      result = formatLeaseRemaining(futureLease, nowInput)
    } catch (e) {
      threw = true
      error = e
    }
    // Record if it throws with BigInt or Symbol when nowSec is adversarial
    if (threw) {
      console.log(`NOTE: formatLeaseRemaining threw with nowSec=${String(nowInput)}: ${error?.message}`)
    }
  }
})

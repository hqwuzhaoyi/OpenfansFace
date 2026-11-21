import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {
  formatTaskDuration,
  formatLeaseRemaining,
  getNodeDisplay,
  getAttemptDisplay,
  TASK_STAGE_LABELS,
  TASK_ERROR_LABELS,
} from '../lib/processing-task-model.ts'

test('Objective 1: Model & Types Verification for Task Attempts & Outbox Deduplication', () => {
  // Test attempt formatting and display
  assert.equal(getAttemptDisplay(null), '未知')
  assert.equal(getAttemptDisplay(''), '未知')
  assert.equal(getAttemptDisplay('run-node1-12345678'), 'run-node')

  // Node display fallback
  assert.equal(getNodeDisplay(null), '未知')
  assert.equal(getNodeDisplay(''), '未知')
  assert.equal(getNodeDisplay('node-gpu-1'), 'node-gpu-1')

  // Duration formatting
  assert.equal(formatTaskDuration(null), '未知')
  assert.equal(formatTaskDuration(undefined), '未知')
  assert.equal(formatTaskDuration(450), '450 ms')
  assert.equal(formatTaskDuration(1500), '1.5 秒')
  assert.equal(formatTaskDuration(65000), '1 分 5 秒')

  // Lease remaining
  const now = 1000
  assert.equal(formatLeaseRemaining(null, now), '无有效租约')
  assert.equal(formatLeaseRemaining(0, now), '无有效租约')
  assert.equal(formatLeaseRemaining(now - 10, now), '租约已过期')
  assert.equal(formatLeaseRemaining(now + 45, now), '剩 45 秒')
  assert.equal(formatLeaseRemaining(now + 120, now), '剩 2 分 0 秒')
})

test('Objective 2: Task Queue Component Truth-in-UI & Attempt History Integration', () => {
  const compPath = path.resolve(process.cwd(), 'components/processing/task-queue.tsx')
  assert.ok(fs.existsSync(compPath), 'components/processing/task-queue.tsx must exist')
  const content = fs.readFileSync(compPath, 'utf-8')

  // Must import TaskAttemptRecord
  assert.ok(content.includes('TaskAttemptRecord'), 'Must import TaskAttemptRecord')

  // Must fetch task-attempts when focused task is active
  assert.ok(
    content.includes('/api/processing/task-attempts?vid=') || content.includes('/v1/agent/task-attempts?vid='),
    'Must fetch task-attempts for focused task'
  )

  // Must render attempt history and deduplication badge
  assert.ok(content.includes('尝试与去重'), 'Must have label 尝试与去重')
  assert.ok(content.includes('幂等去重 x'), 'Must render 幂等去重 x badge')
  assert.ok(content.includes('中断回收'), 'Must handle interrupted attempt status')

  // Truth-in-UI: must not hardcode 0 for unknown size or metrics
  assert.ok(content.includes('未知'), 'Must display 未知 for missing/unknown metrics')
})

test('Objective 3: Agent Outbox & Cleanup Module Implementation Checks', () => {
  const cleanupRs = path.resolve(process.cwd(), 'services/file-operations/src/agent/cleanup.rs')
  assert.ok(fs.existsSync(cleanupRs), 'agent/cleanup.rs must exist')
  const cleanupContent = fs.readFileSync(cleanupRs, 'utf-8')

  // Strictly protected TTS service
  assert.ok(cleanupContent.includes('indextts'), 'Must include indextts protection')
  assert.ok(cleanupContent.includes('PROHIBITED_PATTERNS'), 'Must enforce PROHIBITED_PATTERNS')
  assert.ok(cleanupContent.includes('face-gpu.sock'), 'Must clean face-gpu.sock')

  const outboxRs = path.resolve(process.cwd(), 'services/file-operations/src/agent/outbox.rs')
  assert.ok(fs.existsSync(outboxRs), 'agent/outbox.rs must exist')
  const outboxContent = fs.readFileSync(outboxRs, 'utf-8')

  // Exponential backoff and states
  assert.ok(outboxContent.includes('outbox_receipts'), 'Must create outbox_receipts table')
  assert.ok(outboxContent.includes('mark_committed'), 'Must support mark_committed')
  assert.ok(outboxContent.includes('mark_abandoned'), 'Must support mark_abandoned on 409')
  assert.ok(outboxContent.includes('record_transient_failure'), 'Must support exponential backoff')
})

test('Objective 4: Backend Rust Dispatcher and Registry Contracts', () => {
  const dispatcherRs = path.resolve(process.cwd(), 'services/file-operations/src/agent_dispatcher.rs')
  const dispatcherContent = fs.readFileSync(dispatcherRs, 'utf-8')

  assert.ok(dispatcherContent.includes('stale_generation'), 'Must reject stale generations with 409')
  assert.ok(dispatcherContent.includes('attempt_mismatch'), 'Must reject attempt mismatch with 409')
  assert.ok(dispatcherContent.includes('lease_expired'), 'Must reject expired lease with 409')
  assert.ok(dispatcherContent.includes('delivery_count'), 'Must track delivery_count on receipts')
  assert.ok(dispatcherContent.includes('idempotent: Some(true)'), 'Must return idempotent response on duplicates')

  const registryRs = path.resolve(process.cwd(), 'services/file-operations/src/agent_registry.rs')
  const registryContent = fs.readFileSync(registryRs, 'utf-8')

  assert.ok(registryContent.includes('reconcile_node_crash'), 'Must have reconcile_node_crash')
  assert.ok(registryContent.includes('check_liveness_timeouts'), 'Must have check_liveness_timeouts')
  assert.ok(registryContent.includes('reconciliation'), 'Must include reconciliation in response')
})

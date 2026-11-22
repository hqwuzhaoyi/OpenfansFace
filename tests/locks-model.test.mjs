import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isAutoMergeAction,
  isDestructivePersonAction,
  lockBadgeStatus,
  lockStatusRank,
  mergeScopeLocks,
  parseLockSnapshotJson,
  resolveEntityLock,
} from '../lib/locks-model.ts'

test('isDestructivePersonAction identifies delete, merge, and rename person actions only', () => {
  for (const action of [
    'delete-person',
    'batch-delete',
    'merge-person',
    'batch-merge',
    'rename-person',
  ]) {
    assert.equal(isDestructivePersonAction(action), true, action)
  }

  for (const action of [
    'archive',
    'auto-archive',
    'auto-merge',
    'archive-unarchive',
    'delete-files',
    'review-delete',
    'move-files',
    'rename-file',
    'queue-confirm',
    '',
    undefined,
  ]) {
    assert.equal(isDestructivePersonAction(action), false, String(action))
  }
})

test('isAutoMergeAction recognizes server auto-archive and local auto-merge actions', () => {
  assert.equal(isAutoMergeAction('auto-archive'), true)
  assert.equal(isAutoMergeAction('auto-merge'), true)
  assert.equal(isAutoMergeAction('auto-watermark-apply'), true)
  assert.equal(isAutoMergeAction('archive'), false)
  assert.equal(isAutoMergeAction('merge-person'), false)
  assert.equal(isAutoMergeAction(undefined), false)
})

test('mergeScopeLocks merges local and server locks with recovery_needed > running > queued precedence', () => {
  const local = new Map([
    ['v-local-only', { label: '排队归档', status: 'queued', taskId: 'l1', action: 'archive' }],
    ['v-server-upgrades', { label: '排队合入', status: 'queued', taskId: 'l2', action: 'auto-merge' }],
    ['v-local-ahead', { label: '删除中', status: 'running', taskId: 'l3', action: 'review-delete' }],
    ['v-recovery-beats-running', { label: '归档中', status: 'running', taskId: 'l4', action: 'archive' }],
    ['v-local-recovery-beats-server-running', { label: '待恢复', status: 'recovery_needed', taskId: 'l5', action: 'archive' }],
    ['v-tie-server-wins', { label: '本地合入中', status: 'running', taskId: 'l6', action: 'auto-merge' }],
  ])

  const server = {
    'v-server-only': {
      op_id: 's1',
      action: 'auto-archive',
      status: 'running',
      label: '合入中',
      started_at: 1000,
    },
    'v-server-upgrades': {
      op_id: 's2',
      action: 'auto-archive',
      status: 'running',
      label: '合入中',
      started_at: 1001,
    },
    'v-local-ahead': {
      op_id: 's3',
      action: 'review-delete',
      status: 'queued',
      label: '排队删除',
      started_at: 1002,
    },
    'v-recovery-beats-running': {
      op_id: 's4',
      action: 'archive',
      status: 'recovery_needed',
      label: '待恢复',
      started_at: 1003,
    },
    'v-local-recovery-beats-server-running': {
      op_id: 's5',
      action: 'archive',
      status: 'running',
      label: '归档中',
      started_at: 1004,
    },
    'v-tie-server-wins': {
      op_id: 's6',
      action: 'auto-archive',
      status: 'running',
      label: '合入中',
      started_at: 1005,
    },
  }

  const merged = mergeScopeLocks(local, server)

  // Purity: original local map is untouched
  assert.equal(local.size, 6)
  assert.equal(local.get('v-server-only'), undefined)

  assert.deepEqual(merged.get('v-local-only'), {
    label: '排队归档',
    status: 'queued',
    taskId: 'l1',
    action: 'archive',
    source: 'local',
  })
  assert.deepEqual(merged.get('v-server-only'), {
    label: '合入中',
    status: 'running',
    taskId: 's1',
    action: 'auto-archive',
    source: 'server',
  })
  assert.deepEqual(merged.get('v-server-upgrades'), {
    label: '合入中',
    status: 'running',
    taskId: 's2',
    action: 'auto-archive',
    source: 'server',
  })
  assert.deepEqual(merged.get('v-local-ahead'), {
    label: '删除中',
    status: 'running',
    taskId: 'l3',
    action: 'review-delete',
    source: 'local',
  })
  assert.deepEqual(merged.get('v-recovery-beats-running'), {
    label: '待恢复',
    status: 'recovery_needed',
    taskId: 's4',
    action: 'archive',
    source: 'server',
  })
  assert.deepEqual(merged.get('v-local-recovery-beats-server-running'), {
    label: '待恢复',
    status: 'recovery_needed',
    taskId: 'l5',
    action: 'archive',
    source: 'local',
  })
  assert.deepEqual(merged.get('v-tie-server-wins'), {
    label: '合入中',
    status: 'running',
    taskId: 's6',
    action: 'auto-archive',
    source: 'server',
  })
})

test('mergeScopeLocks handles missing server record and fallback labels', () => {
  const local = new Map([
    ['Alice', { label: '归档中', status: 'running', taskId: 't1', action: 'archive' }],
  ])
  const withoutServer = mergeScopeLocks(local, undefined)
  assert.equal(withoutServer.size, 1)
  assert.equal(withoutServer.get('Alice')?.source, 'local')

  const fallback = mergeScopeLocks(new Map(), {
    Bob: {
      op_id: 'op-bob',
      action: 'delete-person',
      status: 'recovery_needed',
      label: '',
      started_at: 100,
    },
  })
  assert.deepEqual(fallback.get('Bob'), {
    label: '待恢复',
    status: 'recovery_needed',
    taskId: 'op-bob',
    action: 'delete-person',
    source: 'server',
  })
})

test('resolveEntityLock prefers higher status rank and destructive person actions on tie', () => {
  const queuedArchive = { label: '排队归档', status: 'queued', taskId: 'a1', action: 'archive' }
  const runningArchive = { label: '归档中', status: 'running', taskId: 'a2', action: 'archive' }
  const runningDelete = { label: '删除中', status: 'running', taskId: 'd1', action: 'delete-person' }
  const recovery = { label: '待恢复', status: 'recovery_needed', taskId: 'r1', action: 'archive' }

  assert.equal(resolveEntityLock(queuedArchive, runningArchive), runningArchive)
  assert.equal(resolveEntityLock(runningArchive, runningDelete), runningDelete)
  assert.equal(resolveEntityLock(runningDelete, runningArchive), runningDelete)
  assert.equal(resolveEntityLock(runningDelete, recovery), recovery)
  assert.equal(resolveEntityLock(undefined, null), undefined)

  assert.ok(lockStatusRank('recovery_needed') > lockStatusRank('running'))
  assert.ok(lockStatusRank('running') > lockStatusRank('queued'))
  assert.equal(lockBadgeStatus('running'), 'loading')
  assert.equal(lockBadgeStatus('recovery_needed'), 'warning')
  assert.equal(lockBadgeStatus('queued'), 'neutral')
})

test('parseLockSnapshotJson parses valid snapshots and rejects malformed payloads', () => {
  const parsed = parseLockSnapshotJson(
    JSON.stringify({
      revision: 7,
      at: 1700000000000,
      videos: { v1: { op_id: 'op1', action: 'auto-archive', status: 'running', label: '合入中', started_at: 1700000000000 } },
      people: { Alice: { op_id: 'op1', action: 'auto-archive', status: 'running', label: '合入中', started_at: 1700000000000 } },
      active_ops: [{ op_id: 'op1', action: 'auto-archive', status: 'running', label: '合入中', video_count: 1, people: ['Alice'], started_at: 1700000000000 }],
      last_settled: { op_id: 'op0', action: 'archive', outcome: 'complete', people: ['Bob'], vids: ['v0'], settled_at: 1699999999000 },
    }),
  )
  assert.ok(parsed)
  assert.equal(parsed.revision, 7)
  assert.equal(parsed.videos.v1.label, '合入中')
  assert.deepEqual(parsed.last_settled?.people, ['Bob'])
  assert.equal(parseLockSnapshotJson('{not-json'), null)
  assert.equal(parseLockSnapshotJson(JSON.stringify({ videos: {} })), null)
})

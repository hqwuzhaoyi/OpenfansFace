import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  clearLegacyOperationStorage,
  filterExternalServerActiveOps,
  formatServerActiveOpCard,
  isUnadmittedNotStarted,
} from '../lib/background-operations-model.ts'

test('clearLegacyOperationStorage removes legacy operation keys only and tolerates errors', () => {
  const store = new Map([
    ['person-file-operation-v1', '{"id":"d1f15a95"}'],
    ['person-batch-operation-v1', '{"plan":{"id":"89ed5d2c"}}'],
    ['face-archive-operation', 'op-archive'],
    ['wb-theme', 'dark'],
    ['wb-privacy', 'on'],
    ['persons:list', '[]'],
  ])
  clearLegacyOperationStorage({
    removeItem: (key) => store.delete(key),
  })
  assert.equal(store.has('person-file-operation-v1'), false)
  assert.equal(store.has('person-batch-operation-v1'), false)
  assert.equal(store.has('face-archive-operation'), false)
  assert.equal(store.get('wb-theme'), 'dark')
  assert.equal(store.get('wb-privacy'), 'on')
  assert.equal(store.get('persons:list'), '[]')

  assert.doesNotThrow(() => clearLegacyOperationStorage(null))
  assert.doesNotThrow(() =>
    clearLegacyOperationStorage({
      removeItem: () => {
        throw new Error('SecurityError')
      },
    }),
  )
})

test('filterExternalServerActiveOps excludes ops tracked by local tasks via id, plan.id, or operationId', () => {
  const serverActiveOps = [
    { op_id: 'op-local-id', action: 'move-files', status: 'running', label: '移动中', video_count: 2, people: ['A'], started_at: 100 },
    { op_id: 'op-local-plan', action: 'batch-delete', status: 'queued', label: '排队删除', video_count: 5, people: ['B'], started_at: 200 },
    { op_id: 'op-local-archive', action: 'archive', status: 'running', label: '归档中', video_count: 3, people: ['C'], started_at: 300 },
    { op_id: 'op-ext-1', action: 'batch-delete', status: 'running', label: '删除中', video_count: 173, people: ['西萌'], started_at: 400 },
    { op_id: 'op-ext-2', action: 'move-files', status: 'recovery_needed', label: '待恢复', video_count: 4, people: ['小敏儿', 'hamusukee'], started_at: 500 },
  ]
  const localTasks = [
    { id: 'op-local-id', kind: 'person-file' },
    { id: 'wrapper-id', kind: 'person-batch', plan: { id: 'op-local-plan' } },
    { id: 'wrapper-arch', kind: 'archive', operationId: 'op-local-archive' },
  ]
  const external = filterExternalServerActiveOps(serverActiveOps, localTasks)
  assert.deepEqual(
    external.map((op) => op.op_id),
    ['op-ext-1', 'op-ext-2'],
  )
})

test('formatServerActiveOpCard formats batch-delete, move-files, archive, auto-archive, and recovery_needed', () => {
  const delCard = formatServerActiveOpCard({
    op_id: '89ed5d2c-7bb4-46cb-bc0f-26581e03951d',
    action: 'batch-delete',
    status: 'running',
    label: '删除中',
    video_count: 173,
    people: ['西萌 西萌工作室'],
    started_at: 1795452036248,
  })
  assert.equal(delCard.title, '删除人物 · 西萌 西萌工作室')
  assert.equal(delCard.subtitle, '173 个视频')
  assert.equal(delCard.status, 'running')
  assert.equal(delCard.badgeText, '执行中')
  assert.equal(delCard.badgeStatus, 'loading')
  assert.equal(delCard.stageLabel, '删除中')

  const moveCard = formatServerActiveOpCard({
    op_id: 'd1f15a95-e733-4098-9416-d7dc92132bb3',
    action: 'move-files',
    status: 'queued',
    label: '排队移动',
    video_count: 4,
    people: ['小敏儿', 'hamusukee', '第三人'],
    started_at: 1795452160937,
  })
  assert.equal(moveCard.title, '移动视频 · 小敏儿、hamusukee 等 3 人')
  assert.equal(moveCard.subtitle, '4 个视频')
  assert.equal(moveCard.status, 'queued')
  assert.equal(moveCard.badgeText, '排队中')
  assert.equal(moveCard.badgeStatus, 'neutral')

  const archiveCard = formatServerActiveOpCard({
    op_id: 'op-arch',
    action: 'archive',
    status: 'running',
    label: '',
    video_count: 0,
    people: ['小敏儿'],
    started_at: 1000,
  })
  assert.equal(archiveCard.title, '归档 · 小敏儿')
  assert.equal(archiveCard.subtitle, '1 位人物')
  assert.equal(archiveCard.stageLabel, '服务端执行中')

  const autoCard = formatServerActiveOpCard({
    op_id: 'op-auto',
    action: 'auto-archive',
    status: 'recovery_needed',
    label: '',
    video_count: 0,
    people: [],
    started_at: Number.NaN,
  })
  assert.equal(autoCard.title, '自动合入')
  assert.equal(autoCard.subtitle, '服务端同步中')
  assert.equal(autoCard.status, 'recovery_needed')
  assert.equal(autoCard.badgeText, '待核对')
  assert.equal(autoCard.badgeStatus, 'warning')
  assert.equal(autoCard.startedAt, 0)
})

test('isUnadmittedNotStarted detects failed pre-admission only when op is absent from server active_ops', () => {
  const active = [{ op_id: 'op-1', action: 'move-files', status: 'queued', label: '排队移动', video_count: 1, people: [], started_at: 1 }]
  assert.equal(isUnadmittedNotStarted('not_started', 'op-1', active), false)
  assert.equal(isUnadmittedNotStarted('not_started', 'op-missing', active), true)
  assert.equal(isUnadmittedNotStarted('running', 'op-missing', active), false)
  assert.equal(isUnadmittedNotStarted('complete', 'op-missing', active), false)
})

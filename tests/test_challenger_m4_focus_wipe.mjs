import { test } from 'node:test'
import assert from 'node:assert/strict'

test('M4 Focus Wipe & Continuous Inspection Verification', () => {
  const focusedTask = {
    vid: 'vid-m4-focus',
    name: 'creator_matched.mp4',
    path: '/media/creator_matched.mp4',
    stage: 'ocr',
    assigned_node_id: 'node-mac-m4',
    attempt_id: 'att-m4-001',
    creator: '@alice',
    face_skipped: true,
  }

  // State updater logic from components/processing/task-queue.tsx lines 120-126
  const updateFocused = (previous, resp) =>
    previous
      ? (resp.task == null
        ? resp.tasks.find((t) => t.vid === previous.vid) || previous
        : resp.task)
      : null

  // Scenario 1: Background poll with task === undefined (standard API response when no specific task returned)
  const pollRespUndefined = {
    schema_version: 1,
    page: 0,
    limit: 25,
    total: 1,
    tasks: [{ ...focusedTask, stage: 'done' }], // Stage advanced on remote node
    task: undefined,
  }
  const result1 = updateFocused(focusedTask, pollRespUndefined)
  assert.notEqual(result1, null, 'Active drawer selection must NOT be wiped when task is undefined')
  assert.equal(result1.vid, 'vid-m4-focus')
  assert.equal(result1.stage, 'done', 'Active drawer must smoothly receive updated stage')

  // Scenario 2: Background poll with task === null (explicit null from backend)
  const pollRespNull = {
    schema_version: 1,
    page: 0,
    limit: 25,
    total: 1,
    tasks: [{ ...focusedTask, stage: 'done' }],
    task: null,
  }
  const result2 = updateFocused(focusedTask, pollRespNull)
  assert.notEqual(result2, null, 'Active drawer selection must NOT be wiped when task is null')
  assert.equal(result2.vid, 'vid-m4-focus')
  assert.equal(result2.stage, 'done')

  // Scenario 3: Task paged out or filtered out from current page (tasks array is empty)
  const pollRespPagedOut = {
    schema_version: 1,
    page: 1,
    limit: 25,
    total: 30,
    tasks: [],
    task: undefined,
  }
  const result3 = updateFocused(focusedTask, pollRespPagedOut)
  assert.notEqual(result3, null, 'Active drawer selection must be retained even if paged out')
  assert.equal(result3.vid, 'vid-m4-focus')
  assert.equal(result3.assigned_node_id, 'node-mac-m4')

  // Scenario 4: User had nothing selected (previous is null)
  const result4 = updateFocused(null, pollRespUndefined)
  assert.equal(result4, null, 'When no drawer was open, focused remains null')
})

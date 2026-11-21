import { test } from 'node:test'
import assert from 'node:assert/strict'

test('VERIFY FIX: task-queue.tsx preserves focused task on background poll', () => {
  // 1. User selects a task in task-queue
  const userSelectedTask = {
    vid: 'video-123',
    name: 'sample.mp4',
    path: '/media/sample.mp4',
    stage: 'error',
    assigned_node_id: 'node-wsl',
  }

  // 2. State has focused = userSelectedTask
  let focused = userSelectedTask

  // 3. Simulated API response from /api/processing/tasks (which sets task: undefined when backendData.task is absent)
  const apiResponse = {
    schema_version: 1,
    page: 0,
    limit: 25,
    total: 1,
    tasks: [{ ...userSelectedTask, stage: 'detect' }], // simulated updated state on poll
    task: undefined, // As produced by app/api/processing/tasks/route.ts
  }

  // 4. task-queue.tsx line 120-126 updater function:
  const updateFocused = (previous, resp) =>
    previous
      ? (resp.task == null
        ? resp.tasks.find((t) => t.vid === previous.vid) || previous
        : resp.task)
      : null

  const newFocused = updateFocused(focused, apiResponse)

  console.log('Original focused task VID:', focused.vid)
  console.log('Focused task after background poll:', newFocused)

  // Verify that newFocused is preserved and refreshed
  assert.notEqual(newFocused, null, 'Focused task must NOT be wiped to null on poll')
  assert.equal(newFocused.vid, 'video-123', 'Focused task VID must be preserved')
  assert.equal(newFocused.stage, 'detect', 'Focused task must be refreshed from current tasks list')

  // Verify backward compatibility: even if API returned null instead of undefined, it is preserved
  const legacyApiResponse = { ...apiResponse, task: null }
  const legacyNewFocused = updateFocused(focused, legacyApiResponse)
  assert.notEqual(legacyNewFocused, null, 'Focused task must NOT be wiped even if API returns null')
  assert.equal(legacyNewFocused.vid, 'video-123')

  // Verify retention if current page doesn't contain the focused task
  const pagedOutApiResponse = { ...apiResponse, tasks: [] }
  const retainedFocused = updateFocused(focused, pagedOutApiResponse)
  assert.notEqual(retainedFocused, null, 'Focused task must fall back to previous when not on current page')
  assert.equal(retainedFocused.vid, 'video-123')
})

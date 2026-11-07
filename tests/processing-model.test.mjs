import { test } from 'node:test'
import assert from 'node:assert/strict'
import { groupReviewTasks, reviewGroup } from '../lib/processing-model.ts'

test('a video with several identities is counted once in conflicts, never once per candidate', () => {
  const rows = [
    { vid: 'a', manual: 0, data: { kind: 'suggested', suggested: 'Alice' } },
    { vid: 'b', manual: 0, data: { kind: 'conflict', candidates: [{name:'Alice'},{name:'Bob'}] } },
    { vid: 'c', manual: 0, data: { kind: 'none' } },
    { vid: 'd', manual: 1, data: { kind: 'suggested', suggested: 'Bob' } },
  ]
  const groups = groupReviewTasks(rows)
  assert.equal(groups.reduce((n,g) => n+g.count, 0), 4)
  assert.equal(groups.find(g => g.key === 'conflict').count, 1)
  assert.equal(reviewGroup(rows[3]).key, 'manual')
})

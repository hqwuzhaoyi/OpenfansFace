import { test } from 'node:test'
import assert from 'node:assert/strict'
import { reviewClusters } from '../lib/cluster-review.ts'

const original = [{ cid: 'mixed', n_videos: 3, total_gb: 99, vids: [
  { vid: 'confirmed', size: 2 ** 30, conf: 'ok' },
  { vid: 'other-person', size: 2 * 2 ** 30, conf: 'weak' },
  { vid: 'unreviewed', size: 3 * 2 ** 30, conf: 'ok' },
] }]

test('partial naming leaves the other videos available after reload', () => {
  const pending = reviewClusters(original, [{ vid: 'confirmed' }])
  assert.deepEqual(pending[0].vids.map(v => v.vid), ['other-person', 'unreviewed'])
  assert.equal(pending[0].n_videos, 2)
  assert.equal(pending[0].total_gb, 5)
  assert.equal(pending[0].weak_count, 1)
  assert.equal(original[0].vids.length, 3)
})
test('fully queued clusters disappear; removing a queue entry restores its video', () => {
  assert.equal(reviewClusters(original, original[0].vids).length, 0)
  assert.equal(reviewClusters(original, []).flatMap(c => c.vids).length, 3)
})

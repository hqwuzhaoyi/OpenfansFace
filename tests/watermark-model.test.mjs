import { test } from 'node:test'
import assert from 'node:assert/strict'
import { watermarkMatches, watermarkSuggestion, watermarkGroups, filterWatermarkGroups } from '../lib/watermark-model.ts'
import { origin } from '../lib/review-logic.ts'

const candidate = (key, frames = 3) => ({ key, frames, token: key, text: '@' + key, platform: 'unknown', kind: 'account', score: .98, timestamp: 1, evidence: 'a.jpg' })
const mapping = (key, creator) => ({ key, creator, kind: 'person' })

test('cached OCR never follows a replaced, moved, or differently processed video', () => {
  const record = { path: '/media/a.mp4', size: 10, mtime_ns: '123456789', pipeline: 'v1' }
  assert.equal(watermarkMatches(record, record.path, 10, '123456789', 'v1'), true)
  for (const [path, size, time, pipeline] of [['/media/b.mp4', 10, '123456789', 'v1'], [record.path, 11, '123456789', 'v1'], [record.path, 10, '123456790', 'v1'], [record.path, 10, '123456789', 'v2']]) {
    assert.equal(watermarkMatches(record, path, size, time, pipeline), false)
  }
})

test('result lists separate unprocessed/empty/error videos and constrain filename searches before selecting', () => {
  const videos = [
    { vid: 'a', name: 'alpha.mp4', path: '/folder/alpha.mp4', watermark: { status: 'done', candidates: [candidate('account')] } },
    { vid: 'b', name: 'beta.mp4', path: '/folder/beta.mp4', watermark: { status: 'done', candidates: [candidate('account')] } },
    { vid: 'new', name: 'new.mp4', watermark: null },
    { vid: 'empty', name: 'empty.mp4', watermark: { status: 'done', candidates: [] } },
    { vid: 'failed', name: 'failed.mp4', watermark: { status: 'error', candidates: [] } },
  ]
  const groups = watermarkGroups(videos, { account: { kind: 'source' } })
  assert.deepEqual(filterWatermarkGroups(groups, 'results', 'alpha').flatMap(g => g.videos.map(v => v.vid)), ['a'])
  assert.equal(filterWatermarkGroups(groups, 'results', 'account')[0].videos.length, 2)
  assert.equal(filterWatermarkGroups(groups, 'source', '')[0].videos.length, 2)
  assert.equal(filterWatermarkGroups(groups, 'unscanned', '')[0].videos[0].vid, 'new')
  assert.equal(filterWatermarkGroups(groups, 'empty', '')[0].videos[0].vid, 'empty')
  assert.equal(filterWatermarkGroups(groups, 'error', '')[0].videos[0].vid, 'failed')
  assert.equal(origin('watermark', 'account').return_href, '/watermarks')
})

test('only confirmed person mappings suggest a name; source marks and conflicting evidence do not', () => {
  const mappings = { a: mapping('a', '甲'), b: mapping('b', '乙'), source: { kind: 'source' } }
  assert.deepEqual(watermarkSuggestion([candidate('a'), candidate('source')], mappings), { suggested_creator: '甲', conflict: false })
  assert.deepEqual(watermarkSuggestion([candidate('source'), candidate('unknown')], mappings), { suggested_creator: null, conflict: false })
  assert.deepEqual(watermarkSuggestion([candidate('a', 1)], mappings), { suggested_creator: null, conflict: false })
  assert.deepEqual(watermarkSuggestion([candidate('a'), candidate('b')], mappings), { suggested_creator: null, conflict: true })
  assert.deepEqual(watermarkSuggestion([candidate('a')], mappings, ['乙']), { suggested_creator: null, conflict: true })
  assert.deepEqual(watermarkSuggestion([candidate('a')], mappings, ['甲']), { suggested_creator: '甲', conflict: false })
})

test('no-face videos remain in the review, and multi-watermark videos appear in each evidence group', () => {
  const videos = [
    { vid: 'multi', watermark: { status: 'done', candidates: [candidate('a'), candidate('b')] } },
    { vid: 'new', watermark: null },
    { vid: 'empty', watermark: { status: 'done', candidates: [] } },
    { vid: 'failed', watermark: { status: 'error', candidates: [] } },
  ]
  const groups = watermarkGroups(videos, {})
  assert.deepEqual(new Set(groups.map(g => g.key)), new Set(['a', 'b', 'unscanned', 'empty', 'error']))
  assert.equal(groups.find(g => g.key === 'a').videos[0].vid, 'multi')
  assert.equal(groups.find(g => g.key === 'b').videos[0].vid, 'multi')
})

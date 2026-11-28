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

test('face similarity groups use raw most common parent folder name without hash', () => {
  const rows = [
    { vid: 'a1', path: '/mnt/media2/pikpak/My Telegram/From-摸鱼评论区-AgADrBMAAistyVU/1.mov', manual: 0, data: { kind: 'none', face: { unknown_group: '0bdabbf147fdb8bf' } } },
    { vid: 'a2', path: '/mnt/media2/pikpak/My Telegram/From-异世界摸鱼协会-AgADWxAAAsZGyFU/2.mov', manual: 0, data: { kind: 'none', face: { unknown_group: '0bdabbf147fdb8bf' } } },
    { vid: 'a3', path: '/mnt/media2/pikpak/My Telegram/From-摸鱼评论区-AgADrBMAAistyVU/3.mov', manual: 0, data: { kind: 'none', face: { unknown_group: '0bdabbf147fdb8bf' } } },
    { vid: 'b1', path: 'alone.mp4', manual: 0, data: { kind: 'none', face: { unknown_group: 'bare0001' } } },
  ]
  const groups = groupReviewTasks(rows)
  assert.equal(groups.find(g => g.key === 'face:0bdabbf147fdb8bf').label, 'From-摸鱼评论区-AgADrBMAAistyVU')
  assert.equal(groups.find(g => g.key === 'face:bare0001').label, '未命名相似组')
})

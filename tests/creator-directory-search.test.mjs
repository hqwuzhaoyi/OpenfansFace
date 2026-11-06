import { test } from 'node:test'
import assert from 'node:assert/strict'
import { creatorSearchTerms, addFuzzyCreatorProfiles } from '../lib/creator-directory-search.ts'
import { matchWatermarkAccount } from '../lib/creator-directory-model.ts'
import { mergeReviewedCreatorProfiles } from '../lib/creator-directory-references.ts'

const p = (id, handle, name, description = '') => ({ id, handle, name, description, followers: null, synced_at: null })
const search = (profiles, query) => addFuzzyCreatorProfiles(profiles, query)

test('query expansion finds embedded accounts and removes only explicit folder decorations', () => {
  const profiles = [p('1', 'sample_01', '小安')]
  for (const query of ['小安@sample_01', 'sample_01小安', '#sample_01-14192581523534213', 'https://x.com/sample_01', 'https://onlyfans.com/sample_01']) {
    assert.ok(creatorSearchTerms(query).includes('sample_01'), query)
    assert.equal(search(profiles, query)[0].basis, 'unverified-handle', query)
  }
  assert.ok(creatorSearchTerms('sample_07').includes('sample_07'))
  assert.deepEqual(creatorSearchTerms('https://not-a-profile.example/abc'), [])
  assert.deepEqual(creatorSearchTerms('姐姐 Love DOM'), ['姐姐 love dom'])
})

test('traditional characters, emoji, width and styled letters normalize only as tentative name clues', () => {
  const profiles = [p('1', 'piggy01', '胖胖豬🍊'), p('2', 'alice01', '𝓐𝓵𝓲𝓬𝓮')]
  assert.equal(search(profiles, '胖胖猪')[0].basis, 'normalized-name')
  assert.equal(search(profiles, 'ＡＬＩＣＥ')[0].profile.id, '2')
  assert.equal(search(profiles, 'ＡＬＩＣＥ')[0].basis, 'normalized-name')
  assert.equal(profiles[0].name, '胖胖豬🍊')
})

test('bio account references work without fuzzy matching a longer mentioned handle', () => {
  const profiles = [p('1', 'other_01', '小安分身', '日常账号 @sample_01；主页 https://onlyfans.com/sample_01'), p('2', 'other_02', '别人', '@sample_012')]
  const results = search(profiles, 'sample_01小安')
  assert.equal(results.find(m => m.profile.id === '1').basis, 'profile-mention')
  assert.ok(!search(profiles, 'sample_01').some(m => m.profile.id === '2'))
  assert.ok(!results.some(m => m.basis === 'confirmed-account'))
})

test('short common strings do not match arbitrary bio text or the middle of English words', () => {
  const profiles = [p('1', 'mathclub', '数学', '会背九九乘法表'), p('2', 'orange99', '九九橙'), p('3', 'curious', '好奇', 'curiosity'), p('4', 'banana_01', 'Banana'), p('5', 'nana_good', '甲')]
  assert.deepEqual(search(profiles, '九九').map(m => m.profile.id), ['2'])
  assert.deepEqual(search(profiles, 'rio'), [])
  assert.deepEqual(search(profiles, 'nana').map(m => m.profile.id), ['5'])
  for (const query of ['姐姐', 'Love', 'DOM', 'couple']) assert.deepEqual(search(profiles, query), [])
})

test('one-edit account typos stay suggestions while numeric suffix changes and strict watermark lookup stay separate', () => {
  const profiles = [p('1', 'sample_01', '小安')]
  for (const query of ['smaple_01', 'sample01', 'sampel_01']) assert.equal(search(profiles, query)[0].basis, 'similar-account')
  assert.deepEqual(search(profiles, 'sample_02'), [])
  assert.equal(search(profiles, 'sample').length, 1)
  assert.equal(search(profiles, 'sample')[0].basis, 'similar-account')
  assert.deepEqual(matchWatermarkAccount(profiles, 'sample01', 'X'), [])
  assert.deepEqual(matchWatermarkAccount(profiles, 'sample_01', 'Instagram'), [])
})

test('exact and reviewed classifications survive fuzzy ranking, and the function never mutates its inputs', () => {
  const profiles = [p('1', 'sample_01', '胖胖豬🍊'), p('2', 'sample_02', '胖胖豬')]
  const exact = [{ profile: profiles[0], basis: 'confirmed-account', evidence: '已确认规则' }]
  const before = structuredClone({ profiles, exact })
  const results = addFuzzyCreatorProfiles(profiles, '胖胖猪', exact)
  assert.equal(results[0].basis, 'confirmed-account')
  assert.equal(results.length, 2)
  const merged = mergeReviewedCreatorProfiles(profiles, '胖胖猪', results, [{ person: '胖胖猪', profile_id: '2', handle: 'sample_02', confidence: 'candidate', evidence: '已记录重名歧义', reviewed_at: '2026-11-07T00:00:00Z' }])
  assert.equal(merged.find(m => m.profile.id === '2').basis, 'reviewed-candidate')
  assert.deepEqual({ profiles, exact }, before)
})

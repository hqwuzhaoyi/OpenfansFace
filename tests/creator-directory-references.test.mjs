import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseReviewedCreatorLinks, mergeReviewedCreatorProfiles } from '../lib/creator-directory-references.ts'

const profile = (id = '1', handle = 'sample_01') => ({ id, handle, name: '资料昵称', description: '', followers: 1, synced_at: null })
const link = (extra = {}) => ({ person: '本地人物', profile_id: '1', handle: 'sample_01', confidence: 'reference', evidence: '账号字样一致，仍需核对人物。', reviewed_at: '2026-11-07T00:00:00Z', ...extra })
const parse = (links) => parseReviewedCreatorLinks({ version: 1, source: 'nv-pu-sa', links })

test('reviewed clues retain evidence, date and ambiguity without changing automatic results', () => {
  const profiles = [profile(), profile('2', 'sample_02')]
  const automatic = [{ profile: profiles[0], basis: 'same-name', evidence: '同名' }]
  const snapshot = structuredClone(automatic)
  const matches = mergeReviewedCreatorProfiles(profiles, '本地人物', automatic, parse([link(), link({ profile_id: '2', handle: 'sample_02', confidence: 'candidate', evidence: '仅相似昵称' })]))
  assert.deepEqual(matches.map(m => m.basis), ['reviewed-reference', 'reviewed-candidate'])
  assert.equal(matches[0].reviewed_at, '2026-11-07T00:00:00.000Z')
  assert.equal(matches[1].evidence, '仅相似昵称')
  assert.deepEqual(automatic, snapshot)
  assert.deepEqual(mergeReviewedCreatorProfiles(profiles, '另一个人物', [], parse([link()])), [])
})

test('references never follow a recycled handle, missing profile, or unreviewed account rename', () => {
  for (const profiles of [[], [profile('2')], [profile('1', 'new_handle')]]) {
    assert.deepEqual(mergeReviewedCreatorProfiles(profiles, '本地人物', [], parse([link()])), [])
  }
  assert.equal(mergeReviewedCreatorProfiles([profile('1', 'SAMPLE_01')], '本地人物', [], parse([link()])).length, 1)
})

test('confirmed X rules remain authoritative and duplicate clues produce one result per profile', () => {
  const confirmed = { profile: profile(), basis: 'confirmed-account', evidence: '原有已确认规则' }
  const matches = mergeReviewedCreatorProfiles([profile()], '本地人物', [confirmed], parse([link(), link({ confidence: 'candidate' })]))
  assert.deepEqual(matches, [confirmed])
  for (const rows of [[link(), link({ confidence: 'candidate' })], [link({ confidence: 'candidate' }), link()]]) {
    assert.equal(parse(rows).length, 1)
    assert.equal(parse(rows)[0].confidence, 'candidate')
  }
})

test('invalid imports are ignored rather than upgrading unverifiable references', () => {
  for (const data of [null, [], {}, { version: 2, source: 'nv-pu-sa', links: [link()] }, { version: 1, source: 'other', links: [link()] }]) assert.deepEqual(parseReviewedCreatorLinks(data), [])
  assert.deepEqual(parse([null, {}, link({ confidence: 'confirmed' }), link({ profile_id: 1 }), link({ handle: 'invalid.handle' }), link({ reviewed_at: 'invalid date' }), link({ evidence: '' })]), [])
})

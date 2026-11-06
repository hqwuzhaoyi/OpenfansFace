import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseCreatorDirectory, matchPersonProfiles, matchWatermarkAccount, xHandle } from '../lib/creator-directory-model.ts'

const raw = (id = '1', screen_name = 'Alice_01', name = '小雨') => ({ id, screen_name, name, description: '公开简介', followers_count: 42, last_synced_at: '2026-11-05T00:00:00Z' })
const profiles = parseCreatorDirectory([raw(), raw('2', 'Alice_02', '小雨')])
const rule = (platform = 'X', kind = 'person', creator = '本地名字') => ({ key: 'rule', token: 'Alice_01', platform, kind, creator, text: '@Alice_01', confirmed_at: '2026-11-06' })

test('composite person names find complete external nickname fields without rewriting account punctuation', () => {
  const directory = parseCreatorDirectory([raw('10', 'HongKong_Doll', 'HongKongDoll')])
  for (const name of ['HongKongDoll 玩偶姐姐', '玩偶姐姐（HongKongDoll）', 'hongkongdoll｜玩偶姐姐']) {
    const matches = matchPersonProfiles(directory, name, [])
    assert.equal(matches.length, 1)
    assert.equal(matches[0].profile.handle, 'HongKong_Doll')
    assert.equal(matches[0].basis, 'name-part')
    assert.match(matches[0].evidence, /片段/)
  }
  for (const name of ['HongKong 玩偶姐姐', 'HongKongDollExtra 玩偶姐姐', 'HongKong-Doll 玩偶姐姐']) {
    assert.deepEqual(matchPersonProfiles(directory, name, []), [])
  }
  assert.deepEqual(matchWatermarkAccount(directory, 'HongKongDoll', 'X'), [])
})

test('composite name clues stay tentative, preserve multiple candidates, and yield to confirmed rules', () => {
  assert.equal(matchPersonProfiles(profiles, 'Alice_01 小雨姐姐', [])[0].basis, 'unverified-handle')
  const matches = matchPersonProfiles(profiles, '小雨 别称', [])
  assert.equal(matches.length, 2)
  assert.ok(matches.every(m => m.basis === 'name-part'))
  assert.deepEqual(matchPersonProfiles(profiles, 'Alice01 别称', []), [])
  const [confirmed] = matchPersonProfiles(profiles, 'Alice_01 小雨姐姐', [rule('X', 'person', 'Alice_01 小雨姐姐')])
  assert.equal(confirmed.basis, 'confirmed-account')
  assert.deepEqual(matchPersonProfiles(parseCreatorDirectory([raw('3', 'Different', 'A')]), 'A 别称', []), [])
})

test('X handles preserve punctuation and suffixes; arbitrary URLs and post URLs never match', () => {
  for (const input of ['@ALICE_01', 'Alice_01', 'https://x.com/Alice_01', 'https://twitter.com/Alice_01/']) assert.equal(xHandle(input), 'alice_01')
  for (const input of ['alice.01', 'alice-01', 'alice_01 extra', 'https://evil.example/Alice_01', 'https://x.com/Alice_01/status/1', 'https://x.com.evil/Alice_01', 'Alice_01?x=1']) assert.equal(xHandle(input), null)
  assert.equal(matchWatermarkAccount(profiles, 'Alice_0', 'X').length, 0)
  assert.equal(matchWatermarkAccount(profiles, 'Alice01', 'X').length, 0)
})

test('only X rules establish an account association; unknown platform is explicitly tentative', () => {
  assert.equal(matchPersonProfiles(profiles, '本地名字', [rule()])[0].basis, 'confirmed-account')
  assert.equal(matchPersonProfiles(profiles, '本地名字', [rule('unknown')])[0].basis, 'unverified-handle')
  for (const platform of ['Instagram', 'Telegram', 'OnlyFans', 'Fansly']) {
    assert.deepEqual(matchPersonProfiles(profiles, '本地名字', [rule(platform)]), [])
    assert.deepEqual(matchWatermarkAccount(profiles, 'Alice_01', platform), [])
  }
  assert.deepEqual(matchPersonProfiles(profiles, '本地名字', [rule('X', 'source')]), [])
  assert.deepEqual(matchPersonProfiles(profiles, '本地名字', [rule('X', 'person', '别人')]), [])
  assert.equal(matchWatermarkAccount(profiles, 'Alice_01', 'X')[0].basis, 'x-account')
})

test('duplicate nicknames remain separate tentative results and never become identity rules', () => {
  const rules = [rule()], before = structuredClone(rules)
  const matches = matchPersonProfiles(profiles, '小雨', rules)
  assert.equal(matches.length, 2)
  assert.ok(matches.every(m => m.basis === 'same-name'))
  assert.deepEqual(rules, before)
  assert.deepEqual(matchPersonProfiles(profiles, '小雨姐姐', rules), [])
  assert.equal(matchPersonProfiles(profiles, 'alice_01', [])[0].basis, 'unverified-handle')
  assert.equal(matchPersonProfiles(profiles, 'https://x.com/Alice_01', [])[0].basis, 'x-account')
})

test('confirmed rules win over weaker matches regardless of ordering and duplicate clues', () => {
  for (const rules of [[rule(), rule('unknown')], [rule('unknown'), rule()]]) {
    const matches = matchPersonProfiles(profiles, '本地名字', rules)
    assert.equal(matches.length, 1)
    assert.equal(matches[0].basis, 'confirmed-account')
  }
})

test('malformed responses cannot replace the cache; duplicate identifiers are excluded', () => {
  for (const data of [null, {}, [], [{ id: 1, screen_name: 'Alice' }], [raw(), raw('2', 'ALICE_01')], [raw(), raw('1', 'Other')]]) {
    assert.throws(() => parseCreatorDirectory(data))
  }
  const data = parseCreatorDirectory([raw(), raw('2', 'ALICE_01'), raw('3', 'SafeAccount')])
  assert.deepEqual(data.map(p => p.handle), ['SafeAccount'])
  const [sanitized] = parseCreatorDirectory([{ ...raw(), followers_count: -1, last_synced_at: 'not a date', description: {}, name: null, avatar_url: 'javascript:bad()' }])
  assert.equal(sanitized.followers, null)
  assert.equal(sanitized.synced_at, null)
  assert.equal(sanitized.description, '')
  assert.equal(sanitized.name, 'Alice_01')
  assert.equal('avatar_url' in sanitized, false)
})

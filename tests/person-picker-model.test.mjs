import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  buildPickerRecommendations,
  confirmedWatermarkKeys,
  findSimilarExistingPeople,
  isRememberableWatermark,
  isTelegramTwinMark,
  pickRuleCandidates,
  rankPersonChoices,
  resolveEffectiveWatermarkRule,
  watermarkMatchesPerson,
} from '../lib/person-picker-model.ts'

function makeTask(overrides) {
  return {
    vid: overrides.vid || 'vid-1',
    path: '/mnt/media2/pikpak/My Telegram/test.mp4',
    name: overrides.name || 'test.mp4',
    size: 1000,
    mtime_ns: '100',
    stage: 'review',
    retry_stage: 'face',
    text_attempts: 1,
    face_attempts: 1,
    next_retry: 0,
    priority: 0,
    is_new: 0,
    batch_id: 'b1',
    created_at: 1,
    updated_at: 1,
    manual: 0,
    deferred: 0,
    note: '',
    data: {
      suggested: overrides.suggested ?? null,
      candidates: overrides.candidates || [],
      text_candidates: overrides.text_candidates || [],
      folder: overrides.folder || [],
      watermark: {
        vid: overrides.vid || 'vid-1',
        path: '/mnt/media2/pikpak/My Telegram/test.mp4',
        size: 1000,
        mtime_ns: '100',
        pipeline: 'ocr-v2',
        job_id: 'j1',
        status: 'done',
        checked_at: '2026-11-23T00:00:00Z',
        sampled_frames: 8,
        candidates: overrides.watermark || [],
      },
    },
  }
}

const catalog = [
  { name: 'hime_tsu', rep_thumb: null, aliases: ['Hime', 'onlyhimefans'], recent_at: '2026-11-23T15:51:58Z' },
  { name: 'luna_bunny', rep_thumb: '/thumb?p=luna.jpg', aliases: [] },
  { name: 'LunaAoki', rep_thumb: null, aliases: [] },
  { name: 'kriim_brd', rep_thumb: null, aliases: [] },
  { name: 'saku_j', rep_thumb: null, aliases: [] },
  { name: 'aryminh', rep_thumb: null, aliases: [] },
  { name: 'cocopie0218', rep_thumb: null, aliases: [] },
  { name: 'raikun325', rep_thumb: null, aliases: [] },
  { name: 'unrelated_creator', rep_thumb: null, aliases: [] },
]

test('person:Hime_ recommends hime_tsu via alias and same-video watermark join (Hime_ + Tsu)', () => {
  const task = makeTask({
    suggested: 'Hime_',
    candidates: [
      {
        name: 'Hime_',
        sources: ['watermark'],
        score: null,
        evidence: [
          { name: 'Hime_', source: 'watermark', confirmed: false, detail: '账号尚未确认：fansly.com/Hime_' },
          { name: 'Hime_', source: 'watermark', confirmed: false, detail: '账号尚未确认：fansly.com/Hime' },
        ],
      },
    ],
    text_candidates: [
      { name: 'Hime', source: 'watermark', confirmed: false, detail: '账号尚未确认：fansly.com/Hime' },
      { name: 'Hime_', source: 'watermark', confirmed: false, detail: '账号尚未确认：fansly.com/Hime_' },
    ],
    watermark: [
      { key: 'k1', text: 'fansly.com/Hime', token: 'Hime', platform: 'Fansly', kind: 'account', frames: 6, score: 0.99, timestamp: 1, evidence: 'a.jpg', box: [1183, 988, 1765, 1066] },
      { key: 'k2', text: 'fansly.com/Hime_', token: 'Hime_', platform: 'Fansly', kind: 'account', frames: 3, score: 0.98, timestamp: 2, evidence: 'b.jpg', box: [1179, 981, 1767, 1080] },
      { key: 'k3', text: 'TG@ysjcc', token: 'ysjcc', platform: 'Telegram', kind: 'account', frames: 2, score: 0.98, timestamp: 3, evidence: 'c.jpg', box: [67, 54, 168, 82] },
      { key: 'k4', text: 'Tsu', token: 'Tsu', platform: 'unknown', kind: 'text', frames: 6, score: 0.89, timestamp: 4, evidence: 'd.jpg', box: [1775, 991, 1913, 1064] },
      { key: 'k5', text: 'Hotwife', token: 'Hotwife', platform: 'unknown', kind: 'text', frames: 2, score: 0.94, timestamp: 5, evidence: 'e.jpg', box: [845, 84, 929, 179] },
    ],
  })

  // With alias in catalog
  const withAlias = buildPickerRecommendations([task], catalog, 'Hime_')
  assert.equal(withAlias.related[0]?.name, 'hime_tsu')
  assert.match(withAlias.related[0]?.match_reason || '', /别名命中|文字线索/)
  // Strong existing related person ranks ahead of uncreated direct candidate Hime_
  assert.equal(withAlias.suggestions[0]?.name, 'hime_tsu')
  assert.equal(withAlias.suggestions[1]?.name, 'Hime_')
  assert.equal(withAlias.suggestions[1]?.is_new, true)

  // Even without aliases, Hime_ + Tsu watermark join still matches hime_tsu
  const noAliasCatalog = catalog.map(p => (p.name === 'hime_tsu' ? { ...p, aliases: [] } : p))
  const withoutAlias = buildPickerRecommendations([task], noAliasCatalog, 'Hime_')
  assert.equal(withoutAlias.related[0]?.name, 'hime_tsu')
  assert.match(withoutAlias.related[0]?.match_reason || '', /水印拼接 · Hime_ \+ Tsu/)
  assert.equal(withoutAlias.suggestions[0]?.name, 'hime_tsu')

  // Adjacent watermark join hint is also generated without noisy non-adjacent ones
  assert.ok(withAlias.nameHints.some(h => h.name === 'Hime_Tsu'))
  assert.ok(!withAlias.nameHints.some(h => /Hotwife/.test(h.name) && h.name.includes('_')))
})

test('recalls luna_bunny, kriim_brd, saku_j, aryminh, cocopie0218, and raikun325', () => {
  const lunaTask = makeTask({
    candidates: [{ name: 'luna', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 'l1', text: 'OnlyFans.com/luna', token: 'luna', platform: 'OnlyFans', kind: 'account', frames: 8, score: 0.99, timestamp: 1, evidence: 'l1.jpg', box: [563, 1390, 954, 1430] },
      { key: 'l2', text: 'bunny', token: 'bunny', platform: 'unknown', kind: 'text', frames: 8, score: 0.99, timestamp: 1, evidence: 'l2.jpg', box: [948, 1391, 1075, 1434] },
      { key: 'l3', text: 'luna_bunny', token: 'luna_bunny', platform: 'unknown', kind: 'text', frames: 5, score: 0.95, timestamp: 1, evidence: 'l3.jpg' },
    ],
  })
  const lunaRes = buildPickerRecommendations([lunaTask], catalog, 'luna')
  assert.equal(lunaRes.related[0]?.name, 'luna_bunny')
  assert.equal(lunaRes.suggestions[0]?.name, 'luna_bunny')

  const kriimTask = makeTask({
    candidates: [{ name: 'kriim', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 'k1', text: 'X@kriim', token: 'kriim', platform: 'X', kind: 'account', frames: 2, score: 0.99, timestamp: 1, evidence: 'k1.jpg' },
      { key: 'k2', text: 'brd', token: 'brd', platform: 'unknown', kind: 'text', frames: 4, score: 0.98, timestamp: 1, evidence: 'k2.jpg' },
    ],
  })
  assert.equal(buildPickerRecommendations([kriimTask], catalog).related[0]?.name, 'kriim_brd')

  const sakuTask = makeTask({
    candidates: [{ name: 'sakuj', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 's1', text: 'OnlyFans.com/sakuj', token: 'sakuj', platform: 'OnlyFans', kind: 'account', frames: 3, score: 0.99, timestamp: 1, evidence: 's1.jpg' },
    ],
  })
  const sakuRes = buildPickerRecommendations([sakuTask], catalog)
  assert.equal(sakuRes.related[0]?.name, 'saku_j')
  assert.match(sakuRes.related[0]?.match_reason || '', /去符号一致/)

  const arymiTask = makeTask({
    candidates: [{ name: 'arymi', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 'a1', text: 'OnlyFans.com/arymi', token: 'arymi', platform: 'OnlyFans', kind: 'account', frames: 3, score: 0.99, timestamp: 1, evidence: 'a1.jpg' },
    ],
  })
  assert.ok(buildPickerRecommendations([arymiTask], catalog).related.some(r => r.name === 'aryminh'))

  const cocopTask = makeTask({
    candidates: [{ name: 'cocop', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 'c1', text: 'onlyfans@cocop', token: 'cocop', platform: 'OnlyFans', kind: 'account', frames: 2, score: 0.99, timestamp: 1, evidence: 'c1.jpg' },
      { key: 'c2', text: 'cocopie0218', token: 'cocopie0218', platform: 'unknown', kind: 'text', frames: 2, score: 0.98, timestamp: 1, evidence: 'c2.jpg' },
    ],
  })
  assert.equal(buildPickerRecommendations([cocopTask], catalog).related[0]?.name, 'cocopie0218')

  const raikunTask = makeTask({
    candidates: [{ name: 'raikun3255', sources: ['watermark'], score: null, evidence: [] }],
    watermark: [
      { key: 'r1', text: '@raikun3255', token: '@raikun3255', platform: 'unknown', kind: 'account', frames: 7, score: 0.99, timestamp: 1, evidence: 'r1.jpg' },
    ],
  })
  const raikunRes = buildPickerRecommendations([raikunTask], catalog)
  assert.equal(raikunRes.related[0]?.name, 'raikun325')
  assert.match(raikunRes.related[0]?.match_reason || '', /前缀关联/)
})

test('ignores Telegram watermarks and generic stop words', () => {
    const tgPeople = [
    { name: 'ysjcc', rep_thumb: null },
    { name: 'fansly_official', rep_thumb: null },
  ]
  const task = makeTask({
    watermark: [
      { key: 't1', text: 'TG@ysjcc', token: 'ysjcc', platform: 'Telegram', kind: 'account', frames: 8, score: 0.99, timestamp: 1, evidence: 't1.jpg' },
      { key: 't2', text: 'fansly', token: 'fansly', platform: 'unknown', kind: 'text', frames: 8, score: 0.99, timestamp: 1, evidence: 't2.jpg' },
    ],
  })
  const res = buildPickerRecommendations([task], tgPeople)
  assert.equal(res.related.length, 0)
  assert.equal(res.suggestions.length, 0)
})

test('rankPersonChoices and findSimilarExistingPeople rank symbol-stripped and 1-edit matches', () => {
  assert.equal(rankPersonChoices(catalog, 'Hime')[0]?.name, 'hime_tsu')
  assert.equal(rankPersonChoices(catalog, 'Hime Tsu')[0]?.name, 'hime_tsu')
  assert.equal(rankPersonChoices(catalog, 'sakuj')[0]?.name, 'saku_j')
  assert.equal(rankPersonChoices(catalog, 'raikun3255')[0]?.name, 'raikun325')

  assert.equal(findSimilarExistingPeople(catalog, 'Hime_')[0]?.name, 'hime_tsu')
  assert.equal(findSimilarExistingPeople(catalog, 'sakuj')[0]?.name, 'saku_j')
})

test('promotes face.matches with thumbnails/support ahead of unconfirmed new watermarks and matches watermarks', () => {
  const faceCatalog = [
    ...catalog,
    { name: 'room_103', rep_thumb: '/thumb?p=room103.jpg', aliases: [] },
    { name: '鱼籽储存器', rep_thumb: '/thumb?p=yuzi.jpg', aliases: [] },
  ]
  const conflictTask = {
    ...makeTask({
      candidates: [
        {
          name: 'tpjgdwampd',
          sources: ['watermark'],
          score: null,
          evidence: [{ name: 'tpjgdwampd', source: 'watermark', confirmed: false, detail: '账号尚未确认：Onlyfans.com/tpjgdwampd' }],
        },
      ],
      watermark: [
        { key: 'w1', text: 'Onlyfans.com/tpjgdwampd', token: 'tpjgdwampd', platform: 'OnlyFans', kind: 'account', frames: 3, score: 0.99, timestamp: 1, evidence: 'w1.jpg' },
      ],
    }),
    data: {
      kind: 'conflict',
      suggested: null,
      candidates: [
        {
          name: 'tpjgdwampd',
          sources: ['watermark'],
          score: null,
          evidence: [{ name: 'tpjgdwampd', source: 'watermark', confirmed: false, detail: '账号尚未确认：Onlyfans.com/tpjgdwampd' }],
        },
      ],
      face: {
        face_count: 5,
        thumbs: ['/home/prajna/t0.jpg'],
        cache_reused: true,
        reference_people: 400,
        sampled_frames: 8,
        matches: [
          { name: 'room_103', source: 'face', score: 0.8147, support: 5, thumb: '/home/prajna/m1.jpg', timestamp: 277.663 },
          { name: '鱼籽储存器', source: 'face', score: 0.5146, support: 5, thumb: '/home/prajna/m2.jpg', timestamp: 1244.339 },
        ],
      },
      watermark: {
        vid: 'vid-face',
        path: '/mnt/media2/pikpak/My Telegram/3.mp4',
        size: 1000,
        mtime_ns: '100',
        pipeline: 'ocr-v2',
        job_id: 'j1',
        status: 'done',
        checked_at: '2026-11-23T00:00:00Z',
        sampled_frames: 8,
        candidates: [
          { key: 'w1', text: 'Onlyfans.com/tpjgdwampd', token: 'tpjgdwampd', platform: 'OnlyFans', kind: 'account', frames: 3, score: 0.99, timestamp: 1, evidence: 'w1.jpg' },
        ],
      },
    },
  }

  const res = buildPickerRecommendations([conflictTask], faceCatalog)
  assert.equal(res.suggestions[0]?.name, 'room_103')
  assert.equal(res.suggestions[1]?.name, '鱼籽储存器')
  assert.equal(res.suggestions[2]?.name, 'tpjgdwampd')
  assert.equal(res.suggestions[2]?.is_new, true)
  assert.equal(res.suggestions[0]?.face_thumb, '/thumb?p=%2Fhome%2Fprajna%2Fm1.jpg')
  assert.equal(res.suggestions[0]?.face_support, 5)
  assert.equal(res.suggestions[0]?.face_timestamp, 277.663)
  assert.deepEqual(res.suggestions[0]?.sources, ['face'])
  assert.match(res.suggestions[0]?.match_reason || '', /人脸 0\.815/)

  const himePerson = faceCatalog.find(p => p.name === 'hime_tsu')
  const himeMarks = [
    { text: 'fansly.com/Hime', token: 'Hime', platform: 'Fansly', kind: 'account' },
    { text: 'fansly.com/Hime_', token: 'Hime_', platform: 'Fansly', kind: 'account' },
    { text: 'Tsu', token: 'Tsu', platform: 'unknown', kind: 'text' },
    { text: 'TG@ysjcc', token: 'ysjcc', platform: 'Telegram', kind: 'account' },
    { text: 'other_person', token: 'other_person', platform: 'OnlyFans', kind: 'account' },
  ]
  assert.equal(watermarkMatchesPerson(himeMarks[0], himePerson, himeMarks), true)
  assert.equal(watermarkMatchesPerson(himeMarks[1], himePerson, himeMarks), true)
  assert.equal(watermarkMatchesPerson(himeMarks[2], himePerson, himeMarks), true)
  assert.equal(watermarkMatchesPerson(himeMarks[3], himePerson, himeMarks), false)
  assert.equal(watermarkMatchesPerson(himeMarks[4], himePerson, himeMarks), false)
})

test('pickRuleCandidates ranks specific matching watermarks first, excludes confirmed ones, and omits unrelated marks', () => {
  const himePerson = { name: 'hime_tsu', aliases: ['Hime'] }
  const commonMarks = [
    { key: 'k-short', text: 'fansly.com/Hime', token: 'Hime', platform: 'Fansly', kind: 'account', frames: 6 },
    { key: 'k-exact', text: 'fansly.com/Hime_', token: 'Hime_', platform: 'Fansly', kind: 'account', frames: 3 },
    { key: 'k-tg', text: 'TG@ysjcc', token: 'ysjcc', platform: 'Telegram', kind: 'account', frames: 8 },
    { key: 'k-other', text: 'Onlyfans.com/other', token: 'other', platform: 'OnlyFans', kind: 'account', frames: 9 },
  ]
  const taskUnconfirmed = makeTask({
    watermark: commonMarks,
    text_candidates: [
      { name: 'Hime', source: 'watermark', confirmed: false, key: 'k-short' },
      { name: 'Hime_', source: 'watermark', confirmed: false, key: 'k-exact' },
    ],
  })
  const picked = pickRuleCandidates(commonMarks, himePerson, commonMarks, [taskUnconfirmed])
  assert.deepEqual(picked.map(m => m.key), ['k-exact', 'k-short'])

  // Excludes watermark once already confirmed for hime_tsu
  const taskConfirmed = makeTask({
    watermark: commonMarks,
    text_candidates: [
      { name: 'hime_tsu', source: 'watermark', confirmed: true, key: 'k-exact' },
      { name: 'Hime', source: 'watermark', confirmed: false, key: 'k-short' },
    ],
  })
  assert.deepEqual([...confirmedWatermarkKeys([taskConfirmed], 'hime_tsu')], ['k-exact'])
  assert.deepEqual(
    pickRuleCandidates(commonMarks, himePerson, commonMarks, [taskConfirmed]).map(m => m.key),
    ['k-short'],
  )

  // Returns empty list when chosen person (face-matched room_103) doesn't match unrelated watermarks
  assert.deepEqual(
    pickRuleCandidates(
      [{ key: 'k-other', text: 'Onlyfans.com/tpjgdwampd', token: 'tpjgdwampd', platform: 'OnlyFans', kind: 'account', frames: 7 }],
      { name: 'room_103', aliases: [] },
      commonMarks,
      [taskUnconfirmed],
    ),
    [],
  )
})

test('supports kind=text domain watermarks like xiaogoupro.com and suppresses Telegram-twin bare handles like @ZYFLS66', () => {
  const zyCatalog = [
    ...catalog,
    { name: 'xiaogoupro', rep_thumb: null, aliases: [] },
  ]
  const zyMarks = [
    { key: 'k-zy-bare', text: '@ZYFLS66', token: '@ZYFLS66', platform: 'unknown', kind: 'account', frames: 8 },
    { key: 'k-zy-tg', text: 'TG@ZYFLS66', token: 'ZYFLS66', platform: 'Telegram', kind: 'account', frames: 7 },
    { key: 'k-fls', text: '资源福利社', token: '资源福利社', platform: 'unknown', kind: 'text', frames: 8 },
    { key: 'k-vip', text: '会员群自购', token: '会员群自购', platform: 'unknown', kind: 'text', frames: 8 },
    { key: 'k-xg-dom', text: 'xiaogoupro.com', token: 'xiaogoupro.com', platform: 'unknown', kind: 'text', frames: 6 },
    { key: 'k-xg-raw', text: 'xiaogoupro', token: 'xiaogoupro', platform: 'unknown', kind: 'text', frames: 2 },
  ]
  const zyTask = {
    ...makeTask({
      candidates: [
        {
          name: 'ZYFLS66',
          sources: ['watermark'],
          score: null,
          evidence: [{ name: 'ZYFLS66', source: 'watermark', confirmed: false, key: 'k-zy-bare', detail: '账号尚未确认：@ZYFLS66' }],
        },
      ],
      watermark: zyMarks,
    }),
    data: {
      kind: 'suggested',
      suggested: 'ZYFLS66',
      candidates: [
        {
          name: 'ZYFLS66',
          sources: ['watermark'],
          score: null,
          evidence: [{ name: 'ZYFLS66', source: 'watermark', confirmed: false, key: 'k-zy-bare', detail: '账号尚未确认：@ZYFLS66' }],
        },
      ],
      face: {
        face_count: 4,
        thumbs: ['/home/prajna/z0.jpg'],
        cache_reused: true,
        reference_people: 210,
        sampled_frames: 8,
        matches: [{ name: 'xiaogoupro', source: 'face', score: 0.5389, support: 4, thumb: '/home/prajna/xg.jpg', timestamp: 113.6 }],
      },
      watermark: {
        vid: 'vid-zy',
        path: '/mnt/media2/pikpak/My Telegram/1月16日 (4).mp4',
        size: 1000,
        mtime_ns: '100',
        pipeline: 'ocr-v2',
        job_id: 'j-zy',
        status: 'done',
        checked_at: '2026-11-24T00:00:00Z',
        sampled_frames: 8,
        candidates: zyMarks,
      },
    },
  }

  assert.equal(isTelegramTwinMark(zyMarks[0], zyMarks), true)
  assert.equal(isRememberableWatermark(zyMarks[0], zyMarks), false)
  assert.equal(isRememberableWatermark(zyMarks[1], zyMarks), false)
  assert.equal(isRememberableWatermark(zyMarks[4], zyMarks), true)
  assert.equal(isRememberableWatermark(zyMarks[5], zyMarks), true)

  const xiaogou = zyCatalog.find(p => p.name === 'xiaogoupro')
  assert.equal(watermarkMatchesPerson(zyMarks[4], xiaogou, zyMarks), true)
  assert.equal(watermarkMatchesPerson(zyMarks[5], xiaogou, zyMarks), true)

  const common = zyMarks.filter(m => isRememberableWatermark(m, zyMarks))
  assert.deepEqual(
    pickRuleCandidates(common, xiaogou, zyMarks, [zyTask]).map(m => m.key),
    ['k-xg-dom', 'k-xg-raw'],
  )

  const rec = buildPickerRecommendations([zyTask], zyCatalog, 'ZYFLS66')
  assert.equal(rec.suggestions[0]?.name, 'xiaogoupro')
  assert.equal(rec.suggestions.some(s => s.name === 'ZYFLS66'), false)
})

test('resolveEffectiveWatermarkRule checks top matching candidate by default and respects manual override/uncheck', () => {
  const mExact = { key: 'k-exact', text: 'xiaogoupro.com' }
  const mPrefix = { key: 'k-prefix', text: 'xiaogou' }
  const mOther = { key: 'k-other', text: 'studio_mark' }
  const commonMarks = [mExact, mPrefix, mOther]
  const ruleCandidates = [mExact, mPrefix]

  // 1. Default state (undefined) auto-checks ruleCandidates[0]
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: undefined,
      commonMarks,
      ruleCandidates,
      candidateIdx: 0,
    }),
    {
      suggestedMark: mExact,
      effectiveKey: 'k-exact',
      checked: true,
      candidateIndex: 0,
    },
  )

  // 2. Prefix-only candidate is also auto-checked when it is the top candidate
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: undefined,
      commonMarks: [mPrefix],
      ruleCandidates: [mPrefix],
      candidateIdx: 0,
    }),
    {
      suggestedMark: mPrefix,
      effectiveKey: 'k-prefix',
      checked: true,
      candidateIndex: 0,
    },
  )

  // 3. No matching candidates leaves rule unchecked and bar hidden
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: undefined,
      commonMarks: [mOther],
      ruleCandidates: [],
      candidateIdx: 0,
    }),
    {
      suggestedMark: null,
      effectiveKey: '',
      checked: false,
      candidateIndex: -1,
    },
  )

  // 4. Explicit uncheck ('') keeps suggestedMark visible while unchecking effectiveKey
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: '',
      commonMarks,
      ruleCandidates,
      candidateIdx: 0,
    }),
    {
      suggestedMark: mExact,
      effectiveKey: '',
      checked: false,
      candidateIndex: 0,
    },
  )

  // 5. Cycling candidateIdx while checked or unchecked updates suggestedMark cleanly
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: 'k-prefix',
      commonMarks,
      ruleCandidates,
      candidateIdx: 1,
    }),
    {
      suggestedMark: mPrefix,
      effectiveKey: 'k-prefix',
      checked: true,
      candidateIndex: 1,
    },
  )
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: '',
      commonMarks,
      ruleCandidates,
      candidateIdx: 1,
    }),
    {
      suggestedMark: mPrefix,
      effectiveKey: '',
      checked: false,
      candidateIndex: 1,
    },
  )

  // 6. Manual selection of a non-candidate common mark from the drawer
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: 'k-other',
      commonMarks,
      ruleCandidates,
      candidateIdx: 0,
    }),
    {
      suggestedMark: mOther,
      effectiveKey: 'k-other',
      checked: true,
      candidateIndex: -1,
    },
  )

  // 7. Stale manual key no longer in commonMarks falls back to current default candidate
  assert.deepEqual(
    resolveEffectiveWatermarkRule({
      draftWatermarkKey: 'k-missing',
      commonMarks: [mPrefix],
      ruleCandidates: [mPrefix],
      candidateIdx: 0,
    }),
    {
      suggestedMark: mPrefix,
      effectiveKey: 'k-prefix',
      checked: true,
      candidateIndex: 0,
    },
  )
})

import type { ProcessingTask } from './processing-model'

function matchesAllWords(person: { name: string; aliases?: string[] }, query: string): boolean {
  const words = query.trim().normalize('NFKC').toLowerCase().split(/\s+/).filter(Boolean)
  const text = [person.name, ...(person.aliases || [])].join(' ').normalize('NFKC').toLowerCase()
  return words.every(word => text.includes(word))
}

export interface PersonChoice {
  name: string
  rep_thumb: string | null
  aliases?: string[]
  sim?: number
  face_thumb?: string | null
  face_support?: number
  face_timestamp?: number
  sources?: string[]
  recent_at?: string
  match_count?: number
  match_reason?: string
  tier?: 'candidate' | 'related'
  is_new?: boolean
  relevance?: number
}

export interface PersonNameHint {
  name: string
  source: string
}

export interface PickerRecommendationResult {
  suggestions: PersonChoice[]
  related: PersonChoice[]
  nameHints: PersonNameHint[]
}

const STOP_WORDS = new Set([
  'fansly',
  'onlyfans',
  'twitter',
  'x',
  'instagram',
  'ig',
  'patreon',
  'tiktok',
  'telegram',
  'tg',
  'com',
  'net',
  'org',
  'www',
  'http',
  'https',
  'hotwife',
  'coser',
  'mp4',
  'mov',
  'mkv',
  'avi',
  'porn',
  'sex',
  'xxx',
  'nsfw',
  'free',
  'vip',
  'ppv',
  'bts',
  'only',
  'fans',
  'official',
  'backup',
  'video',
  'videos',
  '1080p',
  '720p',
  '4k',
  '60fps',
])

const HAN_RE = /\p{Script=Han}/u

const DOMAIN_SUFFIX_RE = /\.(?:com|net|org|tv|me|cc|io|vip|xyz|top|cn|jp|co|site|online|club|live)$/i

export function stripDomainSuffix(value: string): string {
  return value
    .trim()
    .replace(/^(?:https?:\/\/)?(?:www\.)?/i, '')
    .replace(/^@+/, '')
    .replace(DOMAIN_SUFFIX_RE, '')
    .trim()
}

export function collectTelegramTokens(
  allMarks?: { token?: string; text?: string; platform?: string }[],
): Set<string> {
  const tokens = new Set<string>()
  for (const m of allMarks || []) {
    if (m.platform !== 'Telegram') continue
    const token = normPersonText((m.token || m.text || '').replace(/^@+/, ''))
    if (token) tokens.add(token)
  }
  return tokens
}

export function isTelegramTwinMark(
  mark: { token?: string; text?: string; platform?: string },
  allMarks?: { token?: string; text?: string; platform?: string }[],
): boolean {
  if (mark.platform === 'Telegram') return true
  if (mark.platform && mark.platform !== 'unknown') return false
  const token = normPersonText((mark.token || mark.text || '').replace(/^@+/, ''))
  if (!token) return false
  return collectTelegramTokens(allMarks).has(token)
}

export function isRememberableWatermark(
  mark: { key?: string; kind?: string; platform?: string; token?: string; text?: string },
  allMarks?: { token?: string; text?: string; platform?: string }[],
): boolean {
  if (!mark.key || (mark.kind !== 'account' && mark.kind !== 'text')) return false
  if (isTelegramTwinMark(mark, allMarks)) return false
  if (mark.kind === 'text') {
    return isUsableTextClue(stripDomainSuffix(mark.token || mark.text || ''))
  }
  const token = stripDomainSuffix(mark.token || mark.text || '')
  return Boolean(token && !STOP_WORDS.has(stemPersonText(token)))
}

export function normPersonText(value: string): string {
  return value.trim().normalize('NFKC').toLowerCase()
}

export function stemPersonText(value: string): string {
  return normPersonText(value).replace(/^[_\-.\s]+|[_\-.\s]+$/g, '')
}

export function compactPersonText(value: string): string {
  return normPersonText(value).replace(/[\p{P}\p{S}\s]+/gu, '')
}

export function splitPersonSegments(value: string): string[] {
  return normPersonText(value)
    .split(/[_\-.\s/()（）\[\]【】]+/u)
    .filter(Boolean)
}

function charLen(value: string): number {
  return Array.from(value).length
}

/**
 * Single-edit distance (insert, delete, substitute, adjacent transposition)
 * with digit protection so distinct numeric suffixes do not falsely collide.
 */
export function isOneEditPerson(a: string, b: string): boolean {
  const charsA = Array.from(a)
  const charsB = Array.from(b)
  if (Math.abs(charsA.length - charsB.length) > 1 || a === b) return false

  let oneEdit = false
  if (charsA.length !== charsB.length) {
    const longer = charsA.length > charsB.length ? charsA : charsB
    const shorter = charsA.length > charsB.length ? charsB : charsA
    let i = 0
    while (i < shorter.length && longer[i] === shorter[i]) i++
    oneEdit = longer.slice(i + 1).join('') === shorter.slice(i).join('')
  } else {
    let i = 0
    while (i < charsA.length && charsA[i] === charsB[i]) i++
    const restMatch = charsA.slice(i + 1).join('') === charsB.slice(i + 1).join('')
    const swapMatch =
      i + 1 < charsA.length &&
      charsA[i] === charsB[i + 1] &&
      charsA[i + 1] === charsB[i] &&
      charsA.slice(i + 2).join('') === charsB.slice(i + 2).join('')
    oneEdit = restMatch || swapMatch
  }
  if (!oneEdit) return false

  const digitsA = (a.match(/\d/g) || []).join('')
  const digitsB = (b.match(/\d/g) || []).join('')
  if (digitsA === digitsB) return true
  if (Math.abs(digitsA.length - digitsB.length) === 1 && (a.startsWith(b) || b.startsWith(a))) {
    const longer = a.length > b.length ? a : b
    return /\d$/.test(longer)
  }
  return false
}

function isUsableTextClue(raw: string): boolean {
  const n = normPersonText(raw)
  if (!n || n.includes('/') || n.includes('\\')) return false
  if (/^\d+$/.test(n) || /^\d+[pvg](?:\d+[pvg])*$/i.test(n)) return false
  const s = stemPersonText(n)
  if (!s || STOP_WORDS.has(s) || STOP_WORDS.has(compactPersonText(s))) return false
  if (HAN_RE.test(s)) return charLen(s) >= 2
  return charLen(compactPersonText(s)) >= 2
}

interface ClueItem {
  value: string
  fromTextCandidate: boolean
  box?: number[]
}

interface RelatedHit {
  relevance: number
  reason: string
  matchedTasks: Set<string>
}

function boxesAdjacent(boxA?: number[], boxB?: number[]): boolean {
  if (!boxA || !boxB || boxA.length !== 4 || boxB.length !== 4) return false
  const [ax1, ay1, ax2, ay2] = boxA
  const [bx1, by1, bx2, by2] = boxB
  if (![ax1, ay1, ax2, ay2, bx1, by1, bx2, by2].every(Number.isFinite)) return false
  const heightA = Math.max(1, ay2 - ay1)
  const heightB = Math.max(1, by2 - by1)
  const overlapY = Math.min(ay2, by2) - Math.max(ay1, by1)
  if (overlapY < Math.min(heightA, heightB) * 0.25) return false
  const gap = bx1 - ax2
  const maxGap = Math.max(40, Math.max(heightA, heightB) * 1.5)
  return bx1 >= ax1 && gap >= -20 && gap <= maxGap
}

function timestampValue(recentAt?: string): number {
  if (!recentAt) return 0
  const parsed = Date.parse(recentAt.includes('T') ? recentAt : `${recentAt.replace(' ', 'T')}Z`)
  return Number.isFinite(parsed) ? parsed : 0
}

function validateHintName(raw: string): boolean {
  const trimmed = raw.trim()
  return Boolean(trimmed && trimmed.length <= 120 && !/[\\/\x00-\x1f]/.test(trimmed) && trimmed !== '.' && trimmed !== '..')
}

export function buildPickerRecommendations(
  tasks: ProcessingTask[],
  people: PersonChoice[],
  groupLabel?: string | null,
): PickerRecommendationResult {
  if (!tasks.length) {
    return { suggestions: [], related: [], nameHints: [] }
  }

  const catalogMap = new Map(people.map(p => [p.name, p]))
  const catalogReady = people.length > 0

  const toThumbUrl = (raw?: string | null): string | null => {
    if (!raw) return null
    return raw.startsWith('/thumb?') || raw.startsWith('http') || raw.startsWith('/')
      ? raw.startsWith('/home/') || raw.startsWith('/data/') || raw.startsWith('/mnt/') || raw.startsWith('/private/')
        ? `/thumb?p=${encodeURIComponent(raw)}`
        : raw
      : `/thumb?p=${encodeURIComponent(raw)}`
  }

  // 1. Direct candidates from task.data.candidates and task.data.face.matches
  const directMap = new Map<
    string,
    {
      seenTasks: Set<string>
      sim?: number
      thumb: string | null
      face_thumb: string | null
      face_support?: number
      face_timestamp?: number
      sources: Set<string>
    }
  >()
  const ensureDirect = (name: string, fallbackThumb: string | null, taskVid: string) => {
    let entry = directMap.get(name)
    if (!entry) {
      entry = {
        seenTasks: new Set(),
        sim: undefined,
        thumb: fallbackThumb,
        face_thumb: null,
        sources: new Set<string>(),
      }
      directMap.set(name, entry)
    }
    entry.seenTasks.add(taskVid)
    if (!entry.thumb && fallbackThumb) entry.thumb = fallbackThumb
    return entry
  }
  const offerFaceStats = (
    entry: {
      sim?: number
      face_thumb: string | null
      face_support?: number
      face_timestamp?: number
    },
    score?: number,
    support?: number,
    thumb?: string,
    timestamp?: number,
  ) => {
    const formattedThumb = toThumbUrl(thumb)
    const isBetter = typeof score === 'number' && (entry.sim === undefined || score > entry.sim)
    if (isBetter) {
      entry.sim = score
      if (typeof support === 'number') entry.face_support = support
      if (formattedThumb) entry.face_thumb = formattedThumb
      if (typeof timestamp === 'number') entry.face_timestamp = timestamp
    } else {
      if (!entry.face_thumb && formattedThumb) entry.face_thumb = formattedThumb
      if (entry.face_support === undefined && typeof support === 'number') entry.face_support = support
      if (entry.face_timestamp === undefined && typeof timestamp === 'number') entry.face_timestamp = timestamp
    }
  }

  for (const t of tasks) {
    const faceThumb = toThumbUrl(t.data?.face?.thumbs?.[0])
    const tMarks = t.data?.watermark?.candidates || []
    const tMarkByKey = new Map(tMarks.map(m => [m.key, m]))
    for (const c of t.data?.candidates || []) {
      const onlyUnconfirmedTgTwin =
        (c.evidence?.length || 0) > 0 &&
        c.evidence.every(
          e =>
            e.source === 'watermark' &&
            e.confirmed !== true &&
            Boolean(e.key && tMarkByKey.get(e.key) && isTelegramTwinMark(tMarkByKey.get(e.key)!, tMarks)),
        )
      if (onlyUnconfirmedTgTwin) continue
      const prev = ensureDirect(c.name, faceThumb, t.vid)
      for (const s of c.sources || []) prev.sources.add(s)
      for (const e of c.evidence || []) {
        if (e.source === 'face') {
          offerFaceStats(prev, e.score, e.support, e.thumb, e.timestamp)
        }
      }
      if (typeof (c as { score?: unknown }).score === 'number') {
        offerFaceStats(prev, (c as { score: number }).score)
      }
    }
    for (const m of t.data?.face?.matches || []) {
      if (!m?.name) continue
      const prev = ensureDirect(m.name, faceThumb, t.vid)
      prev.sources.add(m.source || 'face')
      offerFaceStats(prev, m.score, m.support, m.thumb, m.timestamp)
    }
  }

  const directEntries: PersonChoice[] = [...directMap.entries()].map(([name, info]) => {
    const existing = catalogMap.get(name)
    const count = info.seenTasks.size
    const parts: string[] = []
    if (tasks.length > 1) parts.push(`${count}/${tasks.length} 命中`)
    if (info.sources.has('watermark')) parts.push('水印线索')
    if (info.sources.has('folder')) parts.push('目录线索')
    if (info.sim !== undefined) {
      parts.push(
        `人脸 ${info.sim.toFixed(3)}${typeof info.face_support === 'number' ? ` · ${info.face_support} 张脸` : ''}`,
      )
    } else if (info.sources.has('face')) {
      parts.push('人脸匹配')
    }
    const sortScore =
      (info.sources.has('watermark') ? 1_000_000 : 0) + count * 1000 + (info.sim ?? 0) * 100
    return {
      name,
      rep_thumb: existing?.rep_thumb || info.face_thumb || info.thumb || null,
      aliases: existing?.aliases,
      sim: info.sim,
      face_thumb: info.face_thumb,
      face_support: info.face_support,
      face_timestamp: info.face_timestamp,
      sources: [...info.sources],
      recent_at: existing?.recent_at,
      match_count: count,
      match_reason: parts.join(' · ') || `${count}/${tasks.length} 命中`,
      tier: 'candidate',
      is_new: catalogReady && !existing,
      relevance: sortScore,
    }
  })

  const directExisting = directEntries
    .filter(c => !c.is_new)
    .sort((a, b) => (b.relevance || 0) - (a.relevance || 0) || a.name.localeCompare(b.name, 'zh-CN'))
  const directNew = directEntries
    .filter(c => c.is_new)
    .sort((a, b) => (b.relevance || 0) - (a.relevance || 0) || a.name.localeCompare(b.name, 'zh-CN'))

  const directNormNames = new Set(directEntries.map(c => normPersonText(c.name)))
  const directExactNames = new Set(directEntries.map(c => c.name))

  // 2. Collect per-task clues for cross-person recall and nameHints
  const perTaskClues = tasks.map(task => {
    const accountClues: ClueItem[] = []
    const textClues: ClueItem[] = []
    const seenAccount = new Set<string>()
    const seenText = new Set<string>()

    const pushAccount = (raw: string | undefined | null, fromTextCandidate = false, box?: number[]) => {
      if (!raw) return
      const cleaned = raw.replace(/^@+/, '').trim()
      if (!cleaned) return
      const n = normPersonText(cleaned)
      if (!n || STOP_WORDS.has(stemPersonText(n))) return
      const key = `${n}|${fromTextCandidate ? 1 : 0}`
      if (!seenAccount.has(key)) {
        seenAccount.add(key)
        accountClues.push({ value: cleaned, fromTextCandidate, box })
      }
    }

    const pushText = (raw: string | undefined | null, box?: number[]) => {
      if (!raw) return
      const cleaned = raw.replace(/^@+/, '').trim()
      if (!isUsableTextClue(cleaned)) return
      const n = normPersonText(cleaned)
      if (!seenText.has(n)) {
        seenText.add(n)
        textClues.push({ value: cleaned, fromTextCandidate: false, box })
      }
    }

    const tMarks = task.data?.watermark?.candidates || []
    for (const c of task.data?.candidates || []) pushAccount(c.name)
    for (const tc of task.data?.text_candidates || []) pushAccount(tc.name, true)
    for (const f of task.data?.folder || []) pushAccount(f.name)
    for (const wm of tMarks) {
      if (isTelegramTwinMark(wm, tMarks)) continue
      const stripped = stripDomainSuffix(wm.token || wm.text || '')
      if (wm.kind === 'account') {
        pushAccount(stripped || wm.token || wm.text, false, wm.box)
      } else if (wm.kind === 'text') {
        pushText(stripped || wm.token || wm.text, wm.box)
      }
    }
    if (groupLabel) pushAccount(groupLabel)

    return { taskVid: task.vid, accountClues, textClues }
  })

  // 3. Score every existing person not already in direct candidates
  const relatedScored: PersonChoice[] = []
  for (const person of people) {
    if (directExactNames.has(person.name) || directNormNames.has(normPersonText(person.name))) {
      continue
    }
    const targets: { raw: string; isAlias: boolean }[] = [
      { raw: person.name, isAlias: false },
      ...(person.aliases || []).map(a => ({ raw: a, isAlias: true })),
    ]

    let best: RelatedHit | null = null
    const offer = (relevance: number, reason: string, taskVid: string) => {
      if (!best || relevance > best.relevance) {
        best = { relevance, reason, matchedTasks: new Set([taskVid]) }
      } else if (relevance === best.relevance) {
        best.matchedTasks.add(taskVid)
      }
    }

    for (const { taskVid, accountClues, textClues } of perTaskClues) {
      for (const target of targets) {
        const tNorm = normPersonText(target.raw)
        const tCompact = compactPersonText(target.raw)
        const tSegments = splitPersonSegments(target.raw)
        const tHasHan = HAN_RE.test(tNorm)
        const tLatin = /^[a-z0-9_.-]+$/i.test(tCompact)

        // 100: exact name/alias hit in account clues or text_candidates
        for (const clue of accountClues) {
          const cNorm = normPersonText(clue.value)
          if (cNorm && cNorm === tNorm) {
            const label = target.isAlias ? `别名命中 · ${clue.value}` : clue.fromTextCandidate ? `文字线索 · ${clue.value}` : `线索命中 · ${clue.value}`
            offer(100, label, taskVid)
          }
        }

        // 96: same-video watermark join (account + non-TG text fragment)
        if (tCompact) {
          for (const acc of accountClues) {
            const accStem = stemPersonText(acc.value)
            const accCompact = compactPersonText(acc.value)
            if (charLen(accStem) < 3 || !accCompact) continue
            for (const txt of textClues) {
              const txtStem = stemPersonText(txt.value)
              const txtCompact = compactPersonText(txt.value)
              if (charLen(txtStem) < 2 || !txtCompact) continue
              if (accCompact + txtCompact === tCompact || txtCompact + accCompact === tCompact) {
                offer(96, `水印拼接 · ${acc.value} + ${txt.value}`, taskVid)
              }
            }
          }
        }

        // 92: non-stopword text watermark exact match
        for (const txt of textClues) {
          const cNorm = normPersonText(txt.value)
          const cCompact = compactPersonText(txt.value)
          const validTextLen = HAN_RE.test(cNorm) ? charLen(cNorm) >= 2 : charLen(cCompact) >= 4
          if (!validTextLen) continue
          if (cNorm === tNorm || (cCompact && cCompact === tCompact)) {
            offer(92, `水印文字 · ${txt.value}`, taskVid)
          }
        }

        // 90: symbol-stripped compact equality (e.g. sakuj == saku_j, Hime Tsu == hime_tsu)
        if (tCompact) {
          for (const clue of accountClues) {
            const cCompact = compactPersonText(clue.value)
            const validCompactLen = HAN_RE.test(cCompact) ? charLen(cCompact) >= 2 : charLen(cCompact) >= 4
            if (validCompactLen && cCompact === tCompact) {
              offer(90, `去符号一致 · ${clue.value}`, taskVid)
            }
          }
        }

        // 85: word/underscore segment prefix association (e.g. Hime_ -> hime_tsu, luna -> luna_bunny, kriim -> kriim_brd)
        for (const clue of accountClues) {
          const cNorm = normPersonText(clue.value)
          const cStem = stemPersonText(clue.value)
          const hasHan = HAN_RE.test(cStem)
          const segMatch = (charLen(cStem) >= 3 || (hasHan && charLen(cStem) >= 2)) && tSegments[0] === cStem
          const delimPrefix = /[_\-.]$/.test(cNorm) && charLen(cStem) >= 3 && tNorm.startsWith(cNorm)
          if (segMatch || delimPrefix) {
            offer(85, `前缀关联 · ${clue.value}`, taskVid)
          }
        }

        // 78: strong prefix / reverse truncation association (e.g. arymi -> aryminh, cocop -> cocopie0218, raikun3255 -> raikun325)
        for (const clue of accountClues) {
          const cStem = stemPersonText(clue.value)
          const cLen = charLen(cStem)
          const tLen = charLen(tNorm)
          const forward = cLen >= 4 && tNorm.startsWith(cStem) && tLen > 0 && cLen / tLen >= 0.4
          const reverse = tLen >= 5 && cStem.startsWith(tNorm) && cLen > 0 && tLen / cLen >= 0.7
          if (forward || reverse) {
            offer(78, `前缀关联 · ${clue.value}`, taskVid)
          }
        }

        // 70: 1-edit approximate spelling on latin handles >= 6 chars
        if (tLatin && !tHasHan && charLen(tCompact) >= 6) {
          for (const clue of accountClues) {
            const cCompact = compactPersonText(clue.value)
            if (/^[a-z0-9]+$/i.test(cCompact) && charLen(cCompact) >= 6 && isOneEditPerson(cCompact, tCompact)) {
              offer(70, `近似拼写 · ${clue.value}`, taskVid)
            }
          }
        }
      }
    }

    if (best !== null) {
      const hit: RelatedHit = best
      relatedScored.push({
        name: person.name,
        rep_thumb: person.rep_thumb,
        aliases: person.aliases,
        recent_at: person.recent_at,
        match_count: hit.matchedTasks.size,
        match_reason: hit.reason,
        tier: 'related',
        is_new: false,
        relevance: hit.relevance,
      })
    }
  }

  relatedScored.sort(
    (a, b) =>
      (b.relevance || 0) - (a.relevance || 0) ||
      (b.match_count || 0) - (a.match_count || 0) ||
      timestampValue(b.recent_at) - timestampValue(a.recent_at) ||
      a.name.localeCompare(b.name, 'zh-CN'),
  )
  const related = relatedScored.slice(0, 5)
  const strongRelated = related.filter(r => (r.relevance || 0) >= 85)
  const weakRelated = related.filter(r => (r.relevance || 0) < 85)

  const suggestions: PersonChoice[] = [
    ...directExisting,
    ...strongRelated,
    ...directNew,
    ...weakRelated,
  ]

  // 4. Name hints for creating new people (plus box-adjacent join clues)
  const hintMap = new Map<string, string>()
  const addHint = (raw: string | undefined, source: string) => {
    if (!raw) return
    const cleaned = raw.trim()
    if (!validateHintName(cleaned) || hintMap.has(cleaned)) return
    hintMap.set(cleaned, source)
  }

  for (const t of tasks) {
    for (const c of t.data?.candidates || []) addHint(c.name, '候选线索')
    const wms = t.data?.watermark?.candidates || []
    for (const w of wms) {
      if (w.platform === 'Telegram') continue
      const token = (w.token || w.text || '').replace(/^@+/, '').trim()
      if (token.length >= 2 && token.length <= 60) {
        addHint(token, w.platform && w.platform !== 'unknown' ? `${w.platform}水印` : '水印线索')
      }
    }
    // Box-adjacent account + short text join hints (e.g. fansly.com/Hime_ + Tsu -> Hime_Tsu)
    for (const acc of wms) {
      if (acc.platform === 'Telegram' || acc.kind !== 'account') continue
      const accToken = (acc.token || '').replace(/^@+/, '').trim()
      if (charLen(stemPersonText(accToken)) < 3) continue
      for (const txt of wms) {
        if (txt.platform === 'Telegram' || txt.kind !== 'text') continue
        const txtToken = (txt.token || txt.text || '').trim()
        if (!/^[a-z0-9]{2,16}$/i.test(txtToken) || STOP_WORDS.has(txtToken.toLowerCase())) continue
        if (boxesAdjacent(acc.box, txt.box)) {
          const joiner = accToken.endsWith('_') || accToken.endsWith('-') ? '' : '_'
          addHint(`${accToken}${joiner}${txtToken}`, '水印拼接')
        }
      }
    }
    for (const f of t.data?.folder || []) addHint(f.name, '目录线索')
  }

  const nameHints: PersonNameHint[] = [...hintMap.entries()]
    .slice(0, 6)
    .map(([name, source]) => ({ name, source }))

  return { suggestions, related, nameHints }
}

function searchScore(person: { name: string; aliases?: string[] }, query: string): number {
  const qNorm = normPersonText(query)
  if (!qNorm) return 1
  const qCompact = compactPersonText(query)
  const pNorm = normPersonText(person.name)
  const pCompact = compactPersonText(person.name)
  const aliases = person.aliases || []

  if (pNorm === qNorm) return 100
  if (aliases.some(a => normPersonText(a) === qNorm)) return 95
  if (pNorm.startsWith(qNorm) || aliases.some(a => normPersonText(a).startsWith(qNorm))) return 88

  if (qCompact && (pCompact === qCompact || aliases.some(a => compactPersonText(a) === qCompact))) {
    return 84
  }
  if (
    charLen(qCompact) >= 2 &&
    (pCompact.includes(qCompact) || aliases.some(a => compactPersonText(a).includes(qCompact)))
  ) {
    return 76
  }
  if (matchesAllWords(person, query)) return 70

  if (/^[a-z0-9]+$/i.test(qCompact) && charLen(qCompact) >= 5) {
    if (/^[a-z0-9]+$/i.test(pCompact) && isOneEditPerson(qCompact, pCompact)) return 60
    if (aliases.some(a => {
      const ac = compactPersonText(a)
      return /^[a-z0-9]+$/i.test(ac) && isOneEditPerson(qCompact, ac)
    })) {
      return 60
    }
  }
  return 0
}

export function rankPersonChoices<T extends { name: string; aliases?: string[] }>(
  choices: T[],
  query: string,
): T[] {
  const trimmed = query.trim()
  if (!trimmed) return choices
  return choices
    .map((person, index) => ({ person, index, score: searchScore(person, trimmed) }))
    .filter(entry => entry.score > 0)
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map(entry => entry.person)
}

export function findSimilarExistingPeople<T extends { name: string; aliases?: string[] }>(
  people: T[],
  draft: string,
  limit = 3,
): T[] {
  const normalizedDraft = normPersonText(draft)
  const compactDraft = compactPersonText(draft)
  if (charLen(compactDraft || normalizedDraft) < 2) return []

  const nonExact = people.filter(
    p =>
      normPersonText(p.name) !== normalizedDraft &&
      !(p.aliases || []).some(a => normPersonText(a) === normalizedDraft),
  )

  const scored: { person: T; score: number }[] = []
  for (const person of nonExact) {
    let score = searchScore(person, draft)
    if (!score) {
      // Also catch prefix stem matches like draft="Hime_" against "hime_tsu" or "raikun3255" against "raikun325"
      const dStem = stemPersonText(draft)
      const targets = [person.name, ...(person.aliases || [])]
      for (const target of targets) {
        const tNorm = normPersonText(target)
        const tCompact = compactPersonText(target)
        if (charLen(dStem) >= 3 && tNorm.startsWith(dStem)) {
          score = Math.max(score, 78)
        } else if (charLen(tCompact) >= 4 && compactDraft.startsWith(tCompact)) {
          score = Math.max(score, 74)
        }
      }
    }
    if (score > 0) scored.push({ person, score })
  }

  return scored
    .sort((a, b) => b.score - a.score || a.person.name.localeCompare(b.person.name, 'zh-CN'))
    .slice(0, limit)
    .map(entry => entry.person)
}

export function watermarkMatchesPerson(
  mark: { text?: string; token?: string; platform?: string },
  person: { name: string; aliases?: string[] } | string | null | undefined,
  allMarks?: { text?: string; token?: string; kind?: string; platform?: string }[],
): boolean {
  if (!person || isTelegramTwinMark(mark, allMarks)) return false
  const rawToken = stripDomainSuffix(mark.token || mark.text || '')
  const markNorm = normPersonText(rawToken)
  const markStem = stemPersonText(rawToken)
  const markCompact = compactPersonText(rawToken)
  if (!markNorm || !markStem || STOP_WORDS.has(markStem) || STOP_WORDS.has(markCompact)) return false

  const targetPerson = typeof person === 'string' ? { name: person, aliases: [] } : person
  const targets = [targetPerson.name, ...(targetPerson.aliases || [])]
    .map(v => v.trim())
    .filter(Boolean)

  for (const target of targets) {
    const tNorm = normPersonText(target)
    const tCompact = compactPersonText(target)
    const tSegs = splitPersonSegments(target)
    if (!tNorm) continue

    if (markNorm === tNorm) return true
    if (
      tCompact &&
      markCompact &&
      (HAN_RE.test(markCompact) ? charLen(markCompact) >= 2 : charLen(markCompact) >= 4) &&
      markCompact === tCompact
    ) {
      return true
    }

    const segMatch =
      (charLen(markStem) >= 3 || (HAN_RE.test(markStem) && charLen(markStem) >= 2)) &&
      tSegs[0] === markStem
    const delimPrefix = /[_\-.]$/.test(markNorm) && charLen(markStem) >= 3 && tNorm.startsWith(markNorm)
    const strongPrefix =
      charLen(markStem) >= 4 &&
      tNorm.startsWith(markStem) &&
      charLen(tNorm) > 0 &&
      charLen(markStem) / charLen(tNorm) >= 0.4
    if (segMatch || delimPrefix || strongPrefix) return true

    if (tCompact && markCompact && allMarks?.length) {
      for (const other of allMarks) {
        if (other === mark || isTelegramTwinMark(other, allMarks)) continue
        const oRaw = stripDomainSuffix(other.token || other.text || '')
        const oStem = stemPersonText(oRaw)
        const oCompact = compactPersonText(oRaw)
        if (
          charLen(oStem) < 2 ||
          !oCompact ||
          STOP_WORDS.has(oStem) ||
          STOP_WORDS.has(oCompact)
        ) {
          continue
        }
        if (markCompact + oCompact === tCompact || oCompact + markCompact === tCompact) {
          return true
        }
      }
    }
  }
  return false
}

export function confirmedWatermarkKeys(tasks: ProcessingTask[], personName?: string): Set<string> {
  const keys = new Set<string>()
  const wanted = personName ? normPersonText(personName) : ''
  for (const task of tasks) {
    for (const tc of task.data?.text_candidates || []) {
      if (tc?.source !== 'watermark' || tc.confirmed !== true || !tc.key) continue
      if (wanted && normPersonText(tc.name || '') !== wanted) continue
      keys.add(tc.key)
    }
    for (const c of task.data?.candidates || []) {
      const candWanted = !wanted || normPersonText(c.name || '') === wanted
      for (const e of c.evidence || []) {
        if (e?.source !== 'watermark' || e.confirmed !== true || !e.key) continue
        if (wanted && !candWanted && normPersonText(e.name || '') !== wanted) continue
        keys.add(e.key)
      }
    }
  }
  return keys
}

export function pickRuleCandidates<
  T extends { key: string; kind?: string; platform?: string; token?: string; text?: string; frames?: number },
>(
  commonMarks: T[],
  person: { name: string; aliases?: string[] } | null | undefined,
  allMarks: { text?: string; token?: string; kind?: string; platform?: string }[],
  tasks: ProcessingTask[],
): T[] {
  const trimmedName = person?.name?.trim()
  if (!trimmedName || !commonMarks.length) return []
  const confirmedForPerson = confirmedWatermarkKeys(tasks, trimmedName)
  const targetNorm = normPersonText(trimmedName)
  const targetCompacts = new Set(
    [trimmedName, ...(person?.aliases || [])].map(v => compactPersonText(v)).filter(Boolean),
  )

  return commonMarks
    .filter(
      mark =>
        isRememberableWatermark(mark, allMarks) &&
        !confirmedForPerson.has(mark.key) &&
        watermarkMatchesPerson(mark, person, allMarks),
    )
    .sort((a, b) => {
      const aRaw = (a.token || a.text || '').replace(/^@+/, '').trim()
      const bRaw = (b.token || b.text || '').replace(/^@+/, '').trim()
      const aStripped = stripDomainSuffix(aRaw)
      const bStripped = stripDomainSuffix(bRaw)
      const aExact = Number(normPersonText(aStripped) === targetNorm)
      const bExact = Number(normPersonText(bStripped) === targetNorm)
      if (bExact !== aExact) return bExact - aExact

      const aCompact = Number(targetCompacts.has(compactPersonText(aStripped)))
      const bCompact = Number(targetCompacts.has(compactPersonText(bStripped)))
      if (bCompact !== aCompact) return bCompact - aCompact

      const lenDiff = bRaw.length - aRaw.length
      if (lenDiff !== 0) return lenDiff
      return (b.frames ?? 0) - (a.frames ?? 0)
    })
}

export interface ResolvedWatermarkRule<T extends { key: string }> {
  suggestedMark: T | null
  effectiveKey: string
  checked: boolean
  candidateIndex: number
}

export function resolveEffectiveWatermarkRule<T extends { key: string }>(params: {
  draftWatermarkKey: string | undefined
  commonMarks: T[]
  ruleCandidates: T[]
  candidateIdx: number
}): ResolvedWatermarkRule<T> {
  const { draftWatermarkKey, commonMarks, ruleCandidates, candidateIdx } = params
  const clampedIdx = ruleCandidates.length
    ? Math.max(0, Math.min(candidateIdx, ruleCandidates.length - 1))
    : 0
  const defaultMark = ruleCandidates[clampedIdx] ?? null

  if (draftWatermarkKey === '') {
    return {
      suggestedMark: defaultMark,
      effectiveKey: '',
      checked: false,
      candidateIndex: defaultMark ? clampedIdx : -1,
    }
  }

  if (draftWatermarkKey) {
    const explicit = commonMarks.find(m => m.key === draftWatermarkKey)
    if (explicit) {
      return {
        suggestedMark: explicit,
        effectiveKey: explicit.key,
        checked: true,
        candidateIndex: ruleCandidates.findIndex(m => m.key === explicit.key),
      }
    }
  }

  return {
    suggestedMark: defaultMark,
    effectiveKey: defaultMark?.key ?? '',
    checked: Boolean(defaultMark),
    candidateIndex: defaultMark ? clampedIdx : -1,
  }
}

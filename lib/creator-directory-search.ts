import OpenCC from 'opencc-js/t2cn'
import type { CreatorMatch, CreatorProfile } from './creator-directory-model'

// This module is server-only by usage: client components import model types, never this dictionary.
const simplify = OpenCC.Converter({ from: 't', to: 'cn' })
const fold = (text: string) => simplify(text.normalize('NFKC')).toLowerCase().trim()
const displayName = (text: string) => fold(text).replace(/[\p{P}\p{S}\p{M}\p{Cf}\s]+/gu, '')
const stopWords = new Set(['姐姐', '妹妹', '合集', '作品', '原创', '写真', '中文', '官方', '韩国', '台灣', '台湾', '日常', '分身版', '重生版', '一个', '主播', 'onlyfans', 'fansly', 'twitter', 'instagram', 'http', 'https', 'www', 'com', 'nsfw', 'coser', 'couple', 'love', 'dom'])
const accountWord = /^[a-z0-9_]+$/i
const nameBoundary = /[^a-z0-9_.-]/i

/** Query expansion is for discovery only; it never rewrites an account or naming rule. */
export function creatorSearchTerms(input: string, manual = false): string[] {
  const normalized = input.normalize('NFKC').trim().replace(/^#+/, '').replace(/-\d{12,}$/, '')
  // A complete profile URL is one account query, not a source of generic URL fragments.
  const url = normalized.match(/^(?:https?:\/\/)?(?:www\.)?(?:x\.com|twitter\.com|onlyfans\.com|fansly\.com|instagram\.com)\/([a-z0-9_]{1,64})\/?$/i)
  if (/https?:\/\//i.test(normalized) && !url) return []
  const raw = url ? [url[1]] : [normalized,
    ...normalized.split(/[\s()（）\[\]【】|｜,，、;；@#]+/u),
    ...(normalized.match(/[a-z][a-z0-9_.-]*|[\p{Script=Han}]{2,}/giu) || [])]
  return [...new Set(raw.map(fold))].filter(term => term.length >= (manual&&/\p{Script=Han}/u.test(term)?1:2) && term.length <= 100 &&
    !/^\d+$/.test(term) && !/^\d+[pvg](?:\d+[pvg])*$/i.test(term) && !stopWords.has(term)).slice(0, 16)
}

interface IndexedProfile { name: string; compactName: string; handle: string; bio: string; mentions: Set<string> }
const indexCache = new Map<string, IndexedProfile>()
function indexed(profile: CreatorProfile): IndexedProfile {
  const key = JSON.stringify([profile.handle, profile.name, profile.description])
  const old = indexCache.get(key)
  if (old) return old
  const bio = fold(profile.description), mentions = new Set<string>()
  // Explicit mentions and platform URLs are distinct from arbitrary text in the bio.
  for (const match of bio.matchAll(/(?:@|(?:x\.com|twitter\.com|onlyfans\.com|fansly\.com)\/)([a-z0-9_]{1,64})(?![a-z0-9_.-])/g)) mentions.add(match[1])
  const result = { name: fold(profile.name), compactName: displayName(profile.name), handle: profile.handle.toLowerCase(), bio, mentions }
  if (indexCache.size >= 2000) indexCache.clear()
  indexCache.set(key, result)
  return result
}

function containsWord(text: string, term: string): boolean {
  let start = text.indexOf(term)
  while (start !== -1) {
    const end = start + term.length
    if ((start === 0 || nameBoundary.test(text[start - 1])) && (end === text.length || nameBoundary.test(text[end]))) return true
    start = text.indexOf(term, start + 1)
  }
  return false
}

/** One insertion, deletion, substitution, or adjacent transposition. */
function oneEdit(a: string, b: string): boolean {
  if (Math.abs(a.length - b.length) > 1 || a === b) return false
  let i = 0
  while (i < Math.min(a.length, b.length) && a[i] === b[i]) i++
  if (a.length !== b.length) return a.length > b.length ? a.slice(i + 1) === b.slice(i) : a.slice(i) === b.slice(i + 1)
  return a.slice(i + 1) === b.slice(i + 1) ||
    (a[i] === b[i + 1] && a[i + 1] === b[i] && a.slice(i + 2) === b.slice(i + 2))
}

/** Rank fuzzy candidates after exact matches. Return all candidates so the caller can count before capping. */
export function addFuzzyCreatorProfiles(profiles: CreatorProfile[], query: string, exact: CreatorMatch[] = [], manual = false): CreatorMatch[] {
  const terms = creatorSearchTerms(query,manual), preserved = new Set(exact.map(m => m.profile.id))
  const additional: CreatorMatch[] = []
  for (const profile of profiles) {
    if (preserved.has(profile.id)) continue
    const idx = indexed(profile)
    let best: CreatorMatch | undefined
    const offer = (basis: CreatorMatch['basis'], relevance: number, evidence: string) => {
      if (!best || relevance > (best.relevance || 0)) best = { profile, basis, relevance, evidence }
    }
    for (const term of terms) {
      const compact = displayName(term), latin = /^[a-z0-9_.-]+$/i.test(term), han = /\p{Script=Han}/u.test(term)
      if (term === idx.handle) offer('unverified-handle', 100, `查询词「${term}」与完整账号一致，原平台待核对`)
      if (compact && compact === idx.compactName) offer('normalized-name', 96, `统一繁简、字形和装饰字符后，「${term}」与昵称「${profile.name}」一致，待核对`)
      if (idx.mentions.has(term) && term.length >= 4) offer('profile-mention', 88, `简介中的账号或平台链接提及「${term}」；可能是关联账号、推广或来源，待核对`)
      if ((han && term.length >= (manual?1:2) && idx.name.includes(term)) || (latin && term.length >= 3 && containsWord(idx.name, term))) {
        offer('name-contains', term.length >= 3 ? 82 : 65, `昵称「${profile.name}」包含「${term}」，可能重名，待核对`)
      }
      if (latin && term.length >= 4 && accountWord.test(term) && (idx.handle.startsWith(term) || (term.length >= 6 && idx.handle.includes(term)))) {
        offer('similar-account', 72, `账号 @${profile.handle} 包含「${term}」，不是完整账号一致，待核对`)
      }
      // Chinese snippets require at least three characters; arbitrary short bio words are too noisy.
      if ((han && term.length >= 3 && idx.bio.includes(term)) || (latin && term.length >= 5 && containsWord(idx.bio, term))) {
        offer('profile-mention', 60, `简介文字提及「${term}」，不一定代表该账号本人，待核对`)
      }
      if (accountWord.test(term) && term.length >= 6 && (term.match(/\d/g) || []).join('') === (idx.handle.match(/\d/g) || []).join('') && oneEdit(term, idx.handle)) {
        offer('similar-account', 45, `查询词「${term}」与 @${profile.handle} 有一处字符差异；账号仍按不同账号保留，待核对`)
      }
    }
    if (best) additional.push(best)
  }
  additional.sort((a, b) => (b.relevance || 0) - (a.relevance || 0) || a.profile.handle.localeCompare(b.profile.handle))
  return [...exact, ...additional]
}

export const CREATOR_DIRECTORY_SOURCE = 'https://nv-pu-sa.pages.dev/'
export const CREATOR_DIRECTORY_DATA = 'https://img.boomboom.nyc.mn/data/archive.json'

export interface CreatorProfile {
  id: string
  handle: string
  name: string
  description: string
  followers: number | null
  synced_at: string | null
}
export interface CreatorMatch {
  profile: CreatorProfile
  basis: 'confirmed-account' | 'x-account' | 'unverified-handle' | 'same-name' | 'name-part' | 'reviewed-reference' | 'reviewed-candidate'
  evidence: string
  reviewed_at?: string
}
export interface CreatorAccountRule {
  token: string
  platform: string
  kind: 'person' | 'source'
  creator?: string
}
export interface CreatorDirectoryResult {
  matches: CreatorMatch[]
  fetched_at: string
  stale: boolean
  warning?: string
  total: number
  source_url: string
}

/** X handles are case-insensitive. Do not remove punctuation or infer OCR suffixes. */
export function xHandle(value: string): string | null {
  const input = value.trim()
  const match = input.match(/^(?:https?:\/\/)?(?:www\.)?(?:x\.com|twitter\.com)\/([A-Za-z0-9_]{1,15})\/?$/i)
  const handle = match ? match[1] : input.replace(/^@/, '')
  return /^[A-Za-z0-9_]{1,15}$/.test(handle) ? handle.toLowerCase() : null
}

export function parseCreatorDirectory(value: unknown): CreatorProfile[] {
  if (!Array.isArray(value)) throw new Error('账号资料格式不正确')
  const rows: CreatorProfile[] = []
  for (const row of value) {
    if (!row || typeof row !== 'object' || typeof row.screen_name !== 'string' ||
      !/^[A-Za-z0-9_]{1,15}$/.test(row.screen_name) || typeof row.id !== 'string' || !/^\d+$/.test(row.id)) continue
    const synced = typeof row.last_synced_at === 'string' ? Date.parse(row.last_synced_at) : NaN
    rows.push({ id: row.id, handle: row.screen_name,
      name: typeof row.name === 'string' ? row.name.slice(0, 200) : row.screen_name,
      description: typeof row.description === 'string' ? row.description.slice(0, 2000) : '',
      followers: typeof row.followers_count === 'number' && Number.isSafeInteger(row.followers_count) && row.followers_count >= 0 ? row.followers_count : null,
      synced_at: Number.isFinite(synced) ? new Date(synced).toISOString() : null })
  }
  // A duplicated ID or handle is ambiguous; never arbitrarily select the first row.
  const ids = new Map<string, number>(), handles = new Map<string, number>()
  for (const row of rows) {
    ids.set(row.id, (ids.get(row.id) || 0) + 1)
    handles.set(row.handle.toLowerCase(), (handles.get(row.handle.toLowerCase()) || 0) + 1)
  }
  const unique = rows.filter(row => ids.get(row.id) === 1 && handles.get(row.handle.toLowerCase()) === 1)
  if (!unique.length) throw new Error('账号资料为空或没有可核对的有效记录')
  return unique
}

export function matchWatermarkAccount(profiles: CreatorProfile[], token: string, platform: string): CreatorMatch[] {
  if (!['X', 'unknown'].includes(platform)) return []
  const handle = xHandle(token)
  if (!handle) return []
  return profiles.filter(p => p.handle.toLowerCase() === handle).map(profile => ({ profile,
    basis: platform === 'X' ? 'x-account' : 'unverified-handle',
    evidence: platform === 'X' ? `X 水印账号 ${token} 精确一致` : `账号字样 ${token} 一致，原平台待核对` }))
}

/** References only: this never creates identity rules, changes names, or queues videos. */
export function matchPersonProfiles(profiles: CreatorProfile[], name: string, mappings: CreatorAccountRule[]): CreatorMatch[] {
  const matches = new Map<string, CreatorMatch>()
  const normalized = name.trim().normalize('NFC')
  // Split only explicit name separators. Never strip underscores, dots or suffixes.
  const parts = [...new Set(normalized.split(/[\s()（）\[\]【】|｜,，、;；]+/u))]
    .filter(part => part !== normalized && (/^[\x00-\x7f]+$/.test(part) ? part.length >= 3 : [...part].length >= 2))
  for (const profile of profiles) {
    const part = parts.find(part => part.toLowerCase() === profile.name.trim().normalize('NFC').toLowerCase())
    if (part) matches.set(profile.id, { profile, basis: 'name-part',
      evidence: `人物名片段「${part}」与外部昵称「${profile.name}」一致，身份待核对` })
    if (profile.name.trim().normalize('NFC') === normalized) matches.set(profile.id, {
      profile, basis: 'same-name', evidence: `昵称与人物名「${name}」完全一致，身份待核对` })
  }
  for (const part of parts) {
    for (const match of matchWatermarkAccount(profiles, part, 'unknown')) {
      if (matches.get(match.profile.id)?.basis === 'same-name') continue
      matches.set(match.profile.id, { ...match, evidence: `人物名片段「${part}」与 X 账号字样一致，原平台待核对` })
    }
  }
  const explicitX = /^(?:https?:\/\/)?(?:www\.)?(?:x\.com|twitter\.com)\//i.test(name.trim())
  for (const match of matchWatermarkAccount(profiles, name, explicitX ? 'X' : 'unknown')) matches.set(match.profile.id, match)
  for (const mapping of mappings) {
    if (mapping.kind !== 'person' || mapping.creator !== name) continue
    for (const match of matchWatermarkAccount(profiles, mapping.token, mapping.platform)) {
      // Explicit X rules take precedence over platform-unknown account clues.
      if (matches.get(match.profile.id)?.basis === 'confirmed-account') continue
      matches.set(match.profile.id, mapping.platform === 'X' ? { ...match, basis: 'confirmed-account',
        evidence: `已确认的 X 账号规则：${mapping.token} → ${name}` } : match)
    }
  }
  const order = { 'confirmed-account': 0, 'x-account': 1, 'reviewed-reference': 2, 'unverified-handle': 3, 'same-name': 4, 'name-part': 5, 'reviewed-candidate': 6 }
  return [...matches.values()].sort((a, b) => order[a.basis] - order[b.basis] || a.profile.handle.localeCompare(b.profile.handle))
}

import type { CreatorMatch, CreatorProfile } from './creator-directory-model'

export interface ReviewedCreatorLink {
  person: string
  profile_id: string
  handle: string
  confidence: 'reference' | 'candidate'
  evidence: string
  reviewed_at: string
}

/** Private runtime data. These are reviewed textual clues, never identity or naming rules. */
export function parseReviewedCreatorLinks(value: unknown): ReviewedCreatorLink[] {
  if (!value || typeof value !== 'object' || !('version' in value) || value.version !== 1 ||
    !('source' in value) || value.source !== 'nv-pu-sa' || !('links' in value) || !Array.isArray(value.links)) return []
  const links = new Map<string, ReviewedCreatorLink>()
  for (const row of value.links) {
    if (!row || typeof row.person !== 'string' || !row.person.trim() || typeof row.profile_id !== 'string' || !/^\d+$/.test(row.profile_id) ||
      typeof row.handle !== 'string' || !/^[A-Za-z0-9_]{1,15}$/.test(row.handle) ||
      !['reference', 'candidate'].includes(row.confidence) || typeof row.evidence !== 'string' || !row.evidence.trim() || row.evidence.length > 2000 ||
      typeof row.reviewed_at !== 'string' || !Number.isFinite(Date.parse(row.reviewed_at))) continue
    const link: ReviewedCreatorLink = { person: row.person, profile_id: row.profile_id, handle: row.handle,
      confidence: row.confidence, evidence: row.evidence, reviewed_at: new Date(row.reviewed_at).toISOString() }
    const key = JSON.stringify([link.person, link.profile_id])
    // Conflicting imported classifications retain uncertainty instead of upgrading a clue.
    if (!links.has(key) || link.confidence === 'candidate') links.set(key, link)
  }
  return [...links.values()]
}

export function mergeReviewedCreatorProfiles(profiles: CreatorProfile[], name: string, automatic: CreatorMatch[], links: ReviewedCreatorLink[]): CreatorMatch[] {
  const result = new Map(automatic.map(match => [match.profile.id, match]))
  const byId = new Map(profiles.map(profile => [profile.id, profile]))
  for (const link of links) {
    if (link.person !== name) continue
    const profile = byId.get(link.profile_id)
    // Bind both stable ID and observed handle. Renames or recycled handles need a new review.
    if (!profile || profile.handle.toLowerCase() !== link.handle.toLowerCase()) continue
    if (result.get(profile.id)?.basis === 'confirmed-account') continue
    result.set(profile.id, { profile, basis: link.confidence === 'reference' ? 'reviewed-reference' : 'reviewed-candidate',
      evidence: link.evidence, reviewed_at: link.reviewed_at })
  }
  const order = { 'manual-link': -1, 'confirmed-account': 0, 'x-account': 1, 'reviewed-reference': 2, 'unverified-handle': 3, 'same-name': 4, 'name-part': 5, 'reviewed-candidate': 6, 'normalized-name': 7, 'name-contains': 8, 'profile-mention': 8, 'similar-account': 8 }
  return [...result.values()].sort((a, b) => order[a.basis] - order[b.basis] || (b.relevance || 0) - (a.relevance || 0) || a.profile.handle.localeCompare(b.profile.handle))
}

import { NextRequest, NextResponse } from 'next/server'
import path from 'node:path'
import { creatorDirectory } from '@/lib/creator-directory'
import { CREATOR_DIRECTORY_SOURCE, matchPersonProfiles, matchWatermarkAccount, type CreatorAccountRule } from '@/lib/creator-directory-model'
import { BASE } from '@/lib/env'
import { readJson } from '@/lib/jsonio'
import { validateCreator } from '@/lib/review-logic'
import { mergeReviewedCreatorProfiles, parseReviewedCreatorLinks } from '@/lib/creator-directory-references'
import { addFuzzyCreatorProfiles } from '@/lib/creator-directory-search'
import { loadPersonAccounts } from '@/lib/person-accounts'
import { manualAccountMatches, type PersonAccountLink } from '@/lib/person-account-model'
import { canonicalPersonName } from '@/lib/person-names'

export async function GET(req: NextRequest) {
  const params = req.nextUrl.searchParams, name = params.get('name'), token = params.get('token'), platform = params.get('platform'), query = params.get('q')
  if ([name, token, query].filter(value => value !== null).length !== 1 ||
    (query !== null && (query.trim().length < 1 || query.length > 100)) ||
    (token !== null && (!token || token.length > 200 || !['X', 'unknown'].includes(platform || '')))) {
    return NextResponse.json({ error: '请提供人物名、1–100 字的查询词，或 X / 未知平台的完整账号' }, { status: 400 })
  }
  let validName: string | null = null
  try { if (name !== null) validName = canonicalPersonName(validateCreator(name)) }
  catch { return NextResponse.json({ error: '人物名无效' }, { status: 400 }) }
  let manualLinks:PersonAccountLink[]=[]
  try{if(validName!==null)manualLinks=loadPersonAccounts().filter(link=>link.creator===validName)}catch{return NextResponse.json({error:'手动账号关联记录无法读取，请核对后重试'},{status:500})}
  try {
    const { profiles, ...state } = await creatorDirectory()
    const exact = validName !== null
      ? matchPersonProfiles(profiles, validName, Object.values(readJson<Record<string, CreatorAccountRule>>(path.join(BASE, 'watermarks', 'mappings.json'), {})))
      : query !== null ? matchPersonProfiles(profiles, query.trim(), []).map(match => ({ ...match, evidence: match.evidence.replaceAll('人物名', '查询词') }))
        : matchWatermarkAccount(profiles, token!, platform!)
    const automatic = validName !== null || query !== null ? addFuzzyCreatorProfiles(profiles, validName ?? query!.trim(), exact, query!==null) : exact
    const reviewed = validName !== null ? mergeReviewedCreatorProfiles(profiles, validName, automatic,
      parseReviewedCreatorLinks(readJson<unknown>(path.join(BASE, 'external-profiles', 'reviewed-links.json'), null)).map(link=>({...link,person:canonicalPersonName(link.person)}))) : automatic
    const manual=validName!==null?manualAccountMatches(manualLinks,validName,profiles):[],linkedIds=new Set(manual.map(match=>match.profile.id))
    const matches=[...manual,...reviewed.filter(match=>!linkedIds.has(match.profile.id))]
    return NextResponse.json({ ...state, matches: matches.slice(0, 20), match_count: matches.length, total: profiles.length, source_url: CREATOR_DIRECTORY_SOURCE }, { headers: { 'Cache-Control': 'no-store' } })
  } catch {
    if(validName!==null&&manualLinks.length)return NextResponse.json({matches:manualAccountMatches(manualLinks,validName),fetched_at:manualLinks.map(link=>link.source_fetched_at).sort().at(-1),stale:true,warning:'公开资料暂不可用，显示手动关联时保存的资料。',total:0,source_url:CREATOR_DIRECTORY_SOURCE},{headers:{'Cache-Control':'no-store'}})
    return NextResponse.json({ error: '外部账号资料暂不可用，请稍后重试' }, { status: 503, headers: { 'Cache-Control': 'no-store' } })
  }
}

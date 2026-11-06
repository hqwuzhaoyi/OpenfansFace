import { NextRequest, NextResponse } from 'next/server'
import path from 'node:path'
import { creatorDirectory } from '@/lib/creator-directory'
import { CREATOR_DIRECTORY_SOURCE, matchPersonProfiles, matchWatermarkAccount, type CreatorAccountRule } from '@/lib/creator-directory-model'
import { BASE } from '@/lib/env'
import { readJson } from '@/lib/jsonio'
import { validateCreator } from '@/lib/review-logic'
import { mergeReviewedCreatorProfiles, parseReviewedCreatorLinks } from '@/lib/creator-directory-references'

export async function GET(req: NextRequest) {
  const params = req.nextUrl.searchParams, name = params.get('name'), token = params.get('token'), platform = params.get('platform')
  if ((name === null) === (token === null) || (token !== null && (!token || token.length > 200 || !['X', 'unknown'].includes(platform || '')))) {
    return NextResponse.json({ error: '请提供人物名，或 X / 未知平台的完整账号' }, { status: 400 })
  }
  let validName: string | null = null
  try { if (name !== null) validName = validateCreator(name) }
  catch { return NextResponse.json({ error: '人物名无效' }, { status: 400 }) }
  try {
    const { profiles, ...state } = await creatorDirectory()
    const automatic = validName !== null
      ? matchPersonProfiles(profiles, validName, Object.values(readJson<Record<string, CreatorAccountRule>>(path.join(BASE, 'watermarks', 'mappings.json'), {})))
      : matchWatermarkAccount(profiles, token!, platform!)
    const matches = validName !== null ? mergeReviewedCreatorProfiles(profiles, validName, automatic,
      parseReviewedCreatorLinks(readJson<unknown>(path.join(BASE, 'external-profiles', 'reviewed-links.json'), null))) : automatic
    return NextResponse.json({ ...state, matches, total: profiles.length, source_url: CREATOR_DIRECTORY_SOURCE }, { headers: { 'Cache-Control': 'no-store' } })
  } catch {
    return NextResponse.json({ error: '外部账号资料暂不可用，请稍后重试' }, { status: 503, headers: { 'Cache-Control': 'no-store' } })
  }
}

'use client'

import { useEffect, useState } from 'react'
import { api } from '@/lib/face-api'
import { CREATOR_DIRECTORY_SOURCE, type CreatorDirectoryResult, type CreatorMatch } from '@/lib/creator-directory-model'
import './external-profiles.css'

const labels: Record<CreatorMatch['basis'], string> = {
  'confirmed-account': '已确认账号关联', 'x-account': 'X 账号匹配',
  'unverified-handle': '同账号字样 · 待核对平台', 'same-name': '同昵称 · 待核对',
  'name-part': '名称片段一致 · 待核对',
}
const dateText = (value: string) => new Date(value).toLocaleString('zh-CN')

export function ExternalProfiles({ name, token, platform }: { name?: string; token?: string; platform?: string }) {
  const [state, setState] = useState<{ query: string; data?: CreatorDirectoryResult; error?: string } | null>(null)
  const [retry, setRetry] = useState(0)
  const query = name ? new URLSearchParams({ name }).toString()
    : token && ['X', 'unknown'].includes(platform || '') ? new URLSearchParams({ token, platform: platform! }).toString() : ''
  useEffect(() => {
    if (!query) return
    const controller = new AbortController()
    setState(null)
    api<CreatorDirectoryResult>(`/api/persons/external?${query}`, { signal: controller.signal })
      .then(data => { if (!controller.signal.aborted) setState({ query, data }) })
      .catch(error => { if (!controller.signal.aborted) setState({ query, error: (error as Error).message }) })
    return () => controller.abort()
  }, [query, retry])
  if (!query) return null
  // Never display the previous person's result while a new request is in flight.
  const current = state?.query === query ? state : null
  return <section className="external-profiles" aria-label="关联账号资料">
    <header><strong>关联账号资料</strong><a href={CREATOR_DIRECTORY_SOURCE} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">来源：女菩萨 ↗</a></header>
    {!current && <p role="status">正在查询公开账号资料…</p>}
    {current?.error && <p role="status">{current.error} <button type="button" onClick={() => setRetry(n => n + 1)}>重试</button></p>}
    {current?.data && <>
      {current.data.warning && <p role="status">{current.data.warning}</p>}
      {!current.data.matches.length && <p>暂未找到匹配的账号或昵称资料。</p>}
      {current.data.matches.map(({ profile, basis, evidence }) => <article key={profile.id}>
        <div className="external-profile-heading"><a href={`https://x.com/${profile.handle}`} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer"><strong>{profile.name}</strong> <span>@{profile.handle} ↗</span></a><span className="external-profile-basis">{labels[basis]}</span></div>
        {profile.description && <p className="external-profile-bio">{profile.description}</p>}
        <p>{evidence}</p>
        <small>{profile.followers !== null && `${profile.followers.toLocaleString('zh-CN')} 关注者`}{profile.synced_at && ` · 来源更新 ${dateText(profile.synced_at)}`}</small>
      </article>)}
      <footer>仅关联公开账号资料，视频归属仍以核对为准。<br/>资料获取：{dateText(current.data.fetched_at)}{current.data.stale ? ' · 缓存待更新' : ''}</footer>
    </>}
  </section>
}

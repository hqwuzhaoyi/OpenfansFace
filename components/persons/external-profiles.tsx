'use client'

import { useEffect, useId, useState, type ReactNode } from 'react'
import { ArrowUpRight, ChevronRight, Link2, Settings2 } from 'lucide-react'
import { AnimatePresence, motion, useIsPresent } from 'motion/react'
import { Card } from '@/components/beui/motion/card'
import { EASE_OUT } from '@/lib/beui/ease'
import { WorkbenchButton as Button, WorkbenchCount as Count } from './ui'
import { useWorkbenchMotion } from './motion'
import { api } from '@/lib/face-api'
import { CREATOR_DIRECTORY_SOURCE, type CreatorDirectoryResult, type CreatorMatch } from '@/lib/creator-directory-model'
import './external-profiles.css'

const labels: Record<CreatorMatch['basis'], string> = {
  'manual-link': '已手动关联',
  'confirmed-account': '已确认账号关联', 'x-account': 'X 账号匹配',
  'unverified-handle': '同账号字样 · 待核对平台', 'same-name': '同昵称 · 待核对',
  'name-part': '名称片段一致 · 待核对',
  'reviewed-reference': '已核对文字线索', 'reviewed-candidate': '疑似关联 · 待核对',
  'normalized-name': '昵称格式匹配 · 待核对', 'name-contains': '昵称包含 · 待核对',
  'profile-mention': '简介提及 · 待核对', 'similar-account': '账号近似 · 待核对',
}
const dateText = (value: string) => new Date(value).toLocaleString('zh-CN')

export function ExternalProfiles({ name, token, platform,refreshKey=0,onManage,compact=false }: { name?: string; token?: string; platform?: string;refreshKey?:number;onManage?:()=>void;compact?:boolean }) {
  const [state, setState] = useState<{ query: string; data?: CreatorDirectoryResult; error?: string } | null>(null)
  const [retry, setRetry] = useState(0)
  const subject = name ? new URLSearchParams({ name }).toString()
    : token && ['X', 'unknown'].includes(platform || '') ? new URLSearchParams({ token, platform: platform! }).toString() : ''
  const [draft, setDraft] = useState({ subject: '', text: '' }), [search, setSearch] = useState({ subject: '', text: '' })
  const input = draft.subject === subject ? draft.text : '', keyword = search.subject === subject ? search.text : ''
  const query = keyword ? new URLSearchParams({ q: keyword }).toString() : subject
  useEffect(() => {
    if (!query) return
    const controller = new AbortController()
    setState(previous=>previous?.query===query?previous:null)
    api<CreatorDirectoryResult>(`/api/persons/external?${query}`, { signal: controller.signal })
      .then(data => { if (!controller.signal.aborted) setState({ query, data }) })
      .catch(error => { if (!controller.signal.aborted) setState({ query, error: (error as Error).message }) })
    return () => controller.abort()
  }, [query, retry,refreshKey])
  if (!subject) return null
  // Never display the previous person's result while a new request is in flight.
  const current = state?.query === query ? state : null
  if(compact)return current?.data?.matches.length?<CompactProfiles key={subject} data={current.data} onManage={onManage}/>:null
  const content = <section className="external-profiles" aria-label="关联账号资料">
    <header><strong>关联账号资料</strong>{onManage&&<button type="button" onClick={onManage}>管理关联</button>}<a href={CREATOR_DIRECTORY_SOURCE} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">来源：女菩萨 ↗</a></header>
    {!compact&&<form className="external-profile-search" onSubmit={event => { event.preventDefault(); setSearch({ subject, text: input.trim() }) }}>
      <input type="search" aria-label="搜索公开账号资料" placeholder="搜索昵称、账号或别名" value={input} minLength={1} maxLength={100} onChange={event => setDraft({ subject, text: event.target.value })}/>
      <button type="submit" disabled={input.trim().length < 1}>搜索</button>
      {keyword && <button type="button" onClick={() => { setDraft({ subject, text: '' }); setSearch({ subject, text: '' }) }}>返回原查询</button>}
    </form>}
    {keyword && <p>搜索「{keyword}」的资料，仅供查看，未关联到当前人物。</p>}
    {!current && <p role="status">正在查询公开账号资料…</p>}
    {current?.error && <p role="status">{current.error} <button type="button" onClick={() => setRetry(n => n + 1)}>重试</button></p>}
    {current?.data && <>
      {current.data.warning && <p role="status">{current.data.warning}</p>}
      {!current.data.matches.length && <p>暂未找到匹配的账号或昵称资料。</p>}
      {(current.data.match_count || 0) > current.data.matches.length && <p>找到 {current.data.match_count} 条线索，先显示前 {current.data.matches.length} 条；可输入更完整的名字缩小范围。</p>}
      {current.data.matches.map(({ profile, basis, evidence, reviewed_at }) => <article key={profile.id}>
        <div className="external-profile-heading"><a href={`https://x.com/${profile.handle}`} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer"><strong>{profile.name}</strong> <span>@{profile.handle} ↗</span></a><span className="external-profile-basis">{labels[basis]}</span></div>
        {profile.description && <p className="external-profile-bio">{profile.description}</p>}
        <p>{evidence}</p>
        {reviewed_at && <p><small>{basis==='manual-link'?'手动关联':'资料核对'}：{dateText(reviewed_at)}{basis==='manual-link'?'':basis === 'reviewed-candidate' ? ' · 尚未确认账号归属' : ' · 仅核对文字资料'}</small></p>}
        <small>{profile.followers !== null && `${profile.followers.toLocaleString('zh-CN')} 关注者`}{profile.synced_at && ` · 来源更新 ${dateText(profile.synced_at)}`}</small>
      </article>)}
      <footer>仅关联公开账号资料，视频归属仍以核对为准。<br/>资料获取：{dateText(current.data.fetched_at)}{current.data.stale ? ' · 缓存待更新' : ''}</footer>
    </>}
  </section>
  return content
}

function ProfilePanel({id,children}:{id:string;children:ReactNode}){
  const {instant}=useWorkbenchMotion(),present=useIsPresent()
  return <motion.div id={id} role="region" aria-label="账号资料" className="pw-profile-panel" inert={!present} aria-hidden={!present||undefined} initial={instant?false:{opacity:0,transform:'translateY(-4px)'}} animate={{opacity:1,transform:'translateY(0px)'}} exit={{opacity:0,transform:instant?'none':'translateY(-4px)'}} transition={{duration:instant?0:.16,ease:EASE_OUT}}>{children}</motion.div>
}
function CompactProfiles({data,onManage}:{data:CreatorDirectoryResult;onManage?:()=>void}){
  const [open,setOpen]=useState(false),id=useId(),{instant}=useWorkbenchMotion()
  return <section className="pw-profile-disclosure" aria-label="关联账号资料">
    <div className="pw-profile-bar"><Button className="pw-profile-toggle" aria-expanded={open} aria-controls={id} onClick={()=>setOpen(value=>!value)}><Link2 size={14}/><span>账号资料</span><span className="pw-profile-count"><Count value={data.matches.length}/></span><motion.span className="pw-profile-chevron" aria-hidden="true" initial={false} animate={{transform:open?'rotate(90deg)':'rotate(0deg)'}} transition={{duration:instant?0:.16,ease:EASE_OUT}}><ChevronRight size={14}/></motion.span></Button>{onManage&&<Button className="pw-profile-manage" aria-label="管理账号关联" title="管理账号关联" onClick={onManage}><Settings2 size={14}/></Button>}</div>
    <AnimatePresence initial={false}>{open&&<ProfilePanel id={id}>
      {data.warning&&<p className="pw-profile-warning" role="status">{data.warning}</p>}
      <div className="pw-profile-cards">{data.matches.map(({profile,basis,evidence,reviewed_at})=>{
        const linked=['manual-link','confirmed-account'].includes(basis),reference=basis==='reviewed-reference'
        return <Card key={profile.id} size="compact" className="pw-profile-card">
          <a className="pw-profile-account" href={`https://x.com/${profile.handle}`} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer"><span className="pw-profile-avatar" aria-hidden="true">{profile.name.slice(0,1)}</span><span><strong>{profile.name}</strong><small>@{profile.handle}</small></span><ArrowUpRight size={14}/></a>
          <div className="pw-profile-meta"><span className="pw-profile-status" data-linked={linked} title={`${labels[basis]}${reviewed_at?' · '+dateText(reviewed_at):''}`}>{linked?'已关联':reference?'文字线索':'待核对'}</span>{profile.followers!==null&&<span>{profile.followers.toLocaleString('zh-CN')} 关注者</span>}</div>
          {profile.description&&<p className="pw-profile-description">{profile.description}</p>}
          {!linked&&<p className="pw-profile-evidence">{evidence}</p>}
        </Card>
      })}</div>
      <footer className="pw-profile-source"><a href={CREATOR_DIRECTORY_SOURCE} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">女菩萨<ArrowUpRight size={12}/></a><time dateTime={data.fetched_at} title={`资料获取：${dateText(data.fetched_at)}`}>{new Date(data.fetched_at).toLocaleDateString('zh-CN')}{data.stale?' · 待更新':''}</time></footer>
    </ProfilePanel>}</AnimatePresence>
  </section>
}

'use client'
import { useCallback, useEffect, useState } from 'react'
import { Check, Link2, Search, X } from 'lucide-react'
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { api } from '@/lib/face-api'
import type { CreatorDirectoryResult, CreatorMatch } from '@/lib/creator-directory-model'
import type { PersonAccountLink } from '@/lib/person-account-model'
import { WorkbenchDialogContent as DialogContent, WorkbenchButton as Button, WorkbenchInput as Input, WorkbenchBadge as Badge } from './ui'

export function AccountAssociation({name,theme,onClose,onChanged}:{name:string;theme:'light'|'dark';onClose:()=>void;onChanged:()=>void}) {
  const [input,setInput]=useState(name),[search,setSearch]=useState<{kind:'name'|'q';value:string}>({kind:'name',value:name})
  const [links,setLinks]=useState<PersonAccountLink[]>([]),[linksLoading,setLinksLoading]=useState(true),[linksError,setLinksError]=useState('')
  const [data,setData]=useState<CreatorDirectoryResult|null>(null),[loading,setLoading]=useState(true),[searchError,setSearchError]=useState(''),[retry,setRetry]=useState(0)
  const [selected,setSelected]=useState<CreatorMatch|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('')
  const loadLinks=useCallback(async()=>{
    setLinksLoading(true);setLinksError('')
    try{const result=await api<{links:PersonAccountLink[]}>(`/api/persons/accounts?name=${encodeURIComponent(name)}`);setLinks(result.links)}catch(e){setLinksError((e as Error).message)}finally{setLinksLoading(false)}
  },[name])
  useEffect(()=>{void loadLinks()},[loadLinks])
  useEffect(()=>{
    const controller=new AbortController();setLoading(true);setSearchError('');setData(null);setSelected(null)
    api<CreatorDirectoryResult>(`/api/persons/external?${new URLSearchParams({[search.kind]:search.value})}`,{signal:controller.signal})
      .then(next=>{if(!controller.signal.aborted)setData(next)}).catch(e=>{if(!controller.signal.aborted)setSearchError(e.message)}).finally(()=>{if(!controller.signal.aborted)setLoading(false)})
    return()=>controller.abort()
  },[search,retry])
  async function associate(){
    if(!selected||busy)return;setBusy(true);setError('');setNotice('')
    try{
      const result=await api<{link:PersonAccountLink}>('/api/persons/accounts',{method:'POST',body:JSON.stringify({name,profile_id:selected.profile.id,handle:selected.profile.handle})})
      setLinks(previous=>[...previous.filter(link=>link.id!==result.link.id),result.link]);setNotice(`已将 @${result.link.profile.handle} 关联到 ${result.link.creator}`);setSelected(null);onChanged()
    }catch(e){setError((e as Error).message)}finally{setBusy(false)}
  }
  async function remove(link:PersonAccountLink){
    if(busy)return;setBusy(true);setError('');setNotice('')
    try{await api('/api/persons/accounts',{method:'DELETE',body:JSON.stringify({name,id:link.id})});setLinks(previous=>previous.filter(row=>row.id!==link.id));setNotice(`已解除 @${link.profile.handle} 的关联`);onChanged()}
    catch(e){setError((e as Error).message);await loadLinks()}finally{setBusy(false)}
  }
  const linked=new Set(links.map(link=>link.profile.id))
  return <Dialog open onOpenChange={open=>!open&&!busy&&onClose()}><DialogContent className="pw-modal pw-account-dialog" data-theme={theme} showCloseButton={!busy} onEscapeKeyDown={e=>busy&&e.preventDefault()} onInteractOutside={e=>busy&&e.preventDefault()}>
    <DialogTitle>给 {name} 关联账号</DialogTitle><DialogDescription>按昵称、账号或别名搜索，选中后确认关联。资料关联不会自动归名或移动视频。</DialogDescription>
    <div className="pw-account-scroll">
      <section className="pw-account-linked"><h3>已关联 <span>{links.length}</span></h3>
        {linksLoading&&<p role="status">正在读取已关联账号…</p>}{linksError&&<p role="alert">{linksError}<Button onClick={()=>void loadLinks()}>重新加载</Button></p>}
        {!linksLoading&&!linksError&&!links.length&&<p>还没有手动关联的账号。</p>}
        {links.map(link=><div key={link.id} className="pw-linked-row"><div><strong>{link.profile.name}</strong><a href={`https://x.com/${link.profile.handle}`} target="_blank" rel="noreferrer" referrerPolicy="no-referrer">@{link.profile.handle} ↗</a></div><Button disabled={busy} aria-label={`解除 @${link.profile.handle} 的关联`} onClick={()=>void remove(link)}><X size={13}/>解除关联</Button></div>)}
      </section>
      <form className="pw-account-search" onSubmit={event=>{event.preventDefault();if(input.trim()){setNotice('');setError('');setSearch({kind:'q',value:input.trim()})}}}><label className="pw-search"><Search size={15}/><Input aria-label="搜索要关联的账号" placeholder="昵称、@账号、别名或 X 主页链接" value={input} maxLength={100} onChange={e=>setInput(e.target.value)} disabled={busy}/></label><Button className="pw-outline" type="submit" disabled={busy||!input.trim()}>搜索</Button></form>
      {loading&&<p role="status" className="pw-account-hint">正在搜索账号资料…</p>}
      {searchError&&<p role="alert" className="pw-error">{searchError}<Button onClick={()=>setRetry(n=>n+1)}>重试</Button></p>}
      {data?.warning&&<p className="pw-account-hint">{data.warning}</p>}
      {data&&(data.match_count||0)>data.matches.length&&<p className="pw-account-hint">找到 {data.match_count} 条，显示前 {data.matches.length} 条；可输入更完整的关键词缩小范围。</p>}
      {!loading&&data&&!data.matches.length&&<p className="pw-account-hint">没有找到结果，试试账号、别名或名字中的一个片段。</p>}
      {data?.matches.map(match=><article className="pw-account-candidate" data-selected={selected?.profile.id===match.profile.id} key={match.profile.id}>
        <div className="pw-account-heading"><div><strong>{match.profile.name}</strong><a href={`https://x.com/${match.profile.handle}`} target="_blank" rel="noreferrer" referrerPolicy="no-referrer">@{match.profile.handle} ↗</a></div>{linked.has(match.profile.id)?<Badge>已关联</Badge>:<Button className={selected?.profile.id===match.profile.id?'pw-primary':'pw-outline'} disabled={busy||linksLoading||!!linksError} aria-label={`选择 @${match.profile.handle}`} aria-pressed={selected?.profile.id===match.profile.id} onClick={()=>{setSelected(match);setError('');setNotice('')}}>{selected?.profile.id===match.profile.id?<Check size={13}/>:<Link2 size={13}/>}选择此账号</Button>}</div>
        {match.profile.description&&<p className="pw-account-bio">{match.profile.description}</p>}<p className="pw-account-hint">{match.evidence}</p>{match.profile.followers!==null&&<small>{match.profile.followers.toLocaleString('zh-CN')} 关注者</small>}
      </article>)}
    </div>
    <div className="pw-account-bottom">{error&&<p className="pw-error" role="alert">{error}</p>}{notice&&<p className="pw-account-notice" role="status">{notice}</p>}{selected&&<p className="pw-account-confirm">关联 <strong>@{selected.profile.handle}</strong> → <strong>{name}</strong></p>}<div className="pw-modal-actions"><Button disabled={busy} onClick={onClose}>完成</Button><Button className="pw-primary" disabled={busy||!selected||!!linksError||linksLoading||!!selected&&linked.has(selected.profile.id)} onClick={()=>void associate()}>{busy?'正在保存…':`确认关联${selected?' @'+selected.profile.handle:''}`}</Button></div></div>
  </DialogContent></Dialog>
}

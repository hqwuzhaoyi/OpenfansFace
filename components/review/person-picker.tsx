'use client'
import { useState } from 'react'
import { Search, Check } from 'lucide-react'
import { PersonReference } from './person-reference'

export interface PersonChoice { name: string; rep_thumb: string | null; sim?: number; recent_at?:string }
export function PersonPicker({ people, suggestions = [], value, onChange, disabled, loading=false,error='',onRetry }: { people: PersonChoice[]; suggestions?: PersonChoice[]; value: string; onChange: (s: string) => void; disabled?: boolean; loading?:boolean;error?:string;onRetry?:()=>void }) {
  const [q,setQ]=useState(''),[expanded,setExpanded]=useState(false)
  const recent=people.filter(p=>p.recent_at).slice(0,4)
  const combined=[...new Map([...recent,...suggestions,...people].map(p=>[p.name,p])).values()].slice(0,12)
  const choices=q?people.filter(v=>v.name.toLowerCase().includes(q.toLowerCase())):expanded?people:combined
  return <div className="space-y-3">
    <div data-keyboard-hide><p className="text-sm font-medium">选择归给谁</p><p className="mt-1 text-xs text-muted-foreground">点人物只选名字；点击“看对照”可以播放她的已知视频。最后再批量确认。</p></div>
    <label className="flex items-center gap-2 rounded-[8px] border bg-background px-2.5"><Search className="h-4 w-4 text-muted-foreground"/><input value={q} onChange={e=>setQ(e.target.value)} type="search" enterKeyHint="search" autoCapitalize="none" autoCorrect="off" onKeyDown={e=>{if(e.key==='Enter'){e.preventDefault();e.currentTarget.blur()}}} placeholder="搜索全部人物" aria-label="搜索全部人物" className="h-8 min-w-0 flex-1 bg-transparent text-xs outline-none max-md:min-h-11 max-md:text-base"/></label>
    <label className="block space-y-1 text-xs text-muted-foreground"><span>选中人物，或输入新名字</span><input value={value} disabled={disabled} onChange={e=>onChange(e.target.value)} placeholder="人物名" aria-label="归名人物" enterKeyHint="done" autoCorrect="off" autoCapitalize="none" onKeyDown={e=>{if(e.key==='Enter'){e.preventDefault();e.currentTarget.blur()}}} className="h-8 w-full rounded-[8px] border bg-background px-2.5 text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring max-md:min-h-11 max-md:text-base"/></label>
    {!q&&suggestions.length>0&&!expanded&&<p className="text-xs text-muted-foreground">最近使用的人物会排在前面；系统推荐仅供参考。</p>}
    <div className="grid grid-cols-2 gap-2 lg:max-h-64 lg:overflow-y-auto lg:overscroll-contain sm:grid-cols-3">{choices.map(p=><div key={p.name} className={`min-w-0 overflow-hidden rounded-[8px] border ${value===p.name?'border-[var(--wb-success)]/50 bg-[var(--wb-success-soft)]':'bg-background'}`}><button type="button" disabled={disabled} onClick={()=>{onChange(p.name);if(matchMedia('(pointer:coarse)').matches&&document.activeElement instanceof HTMLElement)document.activeElement.blur()}} className="flex min-h-16 w-full items-center gap-2 p-2 text-left" aria-label={`选择人物 ${p.name}`} aria-pressed={value===p.name}>{p.rep_thumb?<img src={p.rep_thumb} alt="" loading="lazy" className="h-10 w-10 shrink-0 rounded-full object-cover"/>:<span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-muted text-xs">{p.name.slice(0,2)}</span>}<span className="min-w-0 flex-1 break-all text-xs font-medium">{p.name}{p.recent_at&&<span className="mt-1 block text-[10px] font-normal text-muted-foreground">最近使用</span>}</span>{value===p.name&&<Check className="h-4 w-4 shrink-0 text-[var(--wb-success)]"/>}</button><div className="border-t p-1"><PersonReference name={p.name} compact/></div></div>)}{!choices.length&&<div className="col-span-full space-y-2 p-2 text-xs text-muted-foreground"><p role={error?'alert':'status'}>{loading?'加载人物…':error?'人物加载失败，可重试或直接输入名字':'没有匹配的人物，也可以直接输入名字'}</p>{error&&onRetry&&<button type="button" onClick={onRetry} className="min-h-8 rounded-[8px] border px-3 max-md:min-h-11">重新加载人物</button>}</div>}</div>
    {!q&&<button type="button" onClick={()=>setExpanded(v=>!v)} className="min-h-8 text-xs text-muted-foreground underline underline-offset-4 max-md:min-h-11">{expanded?'收起全列表':'查看全部人物'}</button>}

  </div>
}

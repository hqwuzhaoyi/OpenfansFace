'use client'
import { useRef, useState } from 'react'
import { Check, Eye, Plus, X, ArrowLeft } from 'lucide-react'
import { matchesPersonQuery } from '@/lib/person-list-model'
import { PersonReference } from './person-reference'
import { WorkspaceSearch } from '@/components/workbench/controls'
import { WorkspaceButton as Button, WorkspaceInput, WorkspaceField, WorkspaceMessage } from '@/components/workbench/inner'

export interface PersonChoice { name: string; rep_thumb: string | null; aliases?:string[];sim?: number; recent_at?:string }
export function PersonPicker({people,suggestions=[],value,onChange,disabled,loading=false,error='',onRetry,onReference,search,onSearchChange}:{search?:string;onSearchChange?:(value:string)=>void;compact?:boolean;people:PersonChoice[];suggestions?:PersonChoice[];value:string;onChange:(value:string)=>void;disabled?:boolean;loading?:boolean;error?:string;onRetry?:()=>void;onReference?:(name:string)=>void}){
  const [localQuery,setLocalQuery]=useState(''),[creating,setCreating]=useState(false),previous=useRef(value),newInput=useRef<HTMLInputElement>(null)
  const query=search??localQuery,setQuery=onSearchChange??setLocalQuery
  const combined=[...new Map([...people.filter(p=>p.recent_at),...suggestions,...people].map(p=>[p.name,p])).values()]
  const choices=combined.filter(p=>matchesPersonQuery(p,query))
  const startCreate=()=>{previous.current=value;setCreating(true);onChange(query.trim());requestAnimationFrame(()=>newInput.current?.focus())}
  const cancelCreate=()=>{onChange(previous.current);setCreating(false)}
  return <section className="ws-person-picker" aria-label="归名人物选择">
    <div className="ws-person-choice"><strong>{creating?'新人物':value||'选择人物'}</strong>{value&&!creating&&<Button variant="ghost" size="icon" aria-label="清空归名人物" title="清空人物" disabled={disabled} onClick={()=>onChange('')}><X size={14}/></Button>}{!creating&&<Button variant="ghost" disabled={disabled} onClick={startCreate}><Plus size={14}/>新建人物</Button>}</div>
    {creating?<div className="ws-create-person"><WorkspaceField label="新人物名称"><WorkspaceInput ref={newInput} value={value} disabled={disabled} maxLength={120} aria-label="归名人物" autoComplete="off" autoCorrect="off" autoCapitalize="none" onChange={e=>onChange(e.target.value)}/></WorkspaceField><Button variant="ghost" disabled={disabled} onClick={cancelCreate}><ArrowLeft size={14}/>返回人物列表</Button></div>:<>
      <WorkspaceSearch value={query} onChange={setQuery} label="搜索全部人物" placeholder="搜索人物或别名…" disabled={disabled}/>
      {error&&<WorkspaceMessage error>{error}{onRetry&&<Button variant="outline" disabled={disabled||loading} onClick={onRetry}>重试</Button>}</WorkspaceMessage>}
      <div className="ws-person-options" role="group" aria-label="可选人物" aria-busy={loading}>{loading&&!people.length?<WorkspaceMessage>读取人物…</WorkspaceMessage>:choices.map(person=><div key={person.name} className="ws-person-option" data-selected={value===person.name}>
        <Button variant="ghost" className="ws-person-select" disabled={disabled} onClick={()=>onChange(person.name)} aria-label={`选择人物 ${person.name}`} aria-pressed={value===person.name}>{person.rep_thumb?<img data-private-media draggable={false} src={person.rep_thumb} alt="" loading="lazy"/>:<span className="ws-person-initial">{person.name.slice(0,1)}</span>}<span className="ws-person-name">{person.name}</span>{value===person.name&&<Check size={14}/>}</Button>
        {onReference?<Button variant="ghost" size="icon" disabled={disabled} aria-label={`看 ${person.name} 的对照视频`} title="查看对照视频" onClick={()=>onReference(person.name)}><Eye size={14}/></Button>:<PersonReference name={person.name} compact/>}
      </div>)}</div>
      {!loading&&!error&&!choices.length&&<WorkspaceMessage>没有匹配的人物。<Button variant="ghost" disabled={disabled} onClick={startCreate}>新建人物</Button></WorkspaceMessage>}
    </>}
  </section>
}

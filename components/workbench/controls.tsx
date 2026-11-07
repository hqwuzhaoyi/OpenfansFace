'use client'

// Composes the existing MIT ReUI Input Group and BEUI Tabs; no new interaction primitives.
// https://reui.io/components/input-group/c-input-group-37
// https://beui.dev/components/motion/tabs
import { useRef, useSyncExternalStore, type ReactNode } from 'react'
import { Search, X } from 'lucide-react'
import { MotionConfig } from 'motion/react'
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '@/components/ui/input-group'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import './workspace.css'

export function WorkspaceSearch({value,onChange,label,placeholder,children,disabled=false}:{value:string;onChange:(value:string)=>void;label:string;placeholder?:string;children?:ReactNode;disabled?:boolean}) {
  const input=useRef<HTMLInputElement>(null)
  const clear=()=>{onChange('');input.current?.focus()}
  return <InputGroup className="ws-search" aria-label={label}>
    <InputGroupAddon><Search size={15} aria-hidden="true"/></InputGroupAddon>
    <InputGroupInput ref={input} type="search" value={value} disabled={disabled} aria-label={label} placeholder={placeholder||label} autoComplete="off" autoCorrect="off" autoCapitalize="none" onChange={e=>onChange(e.target.value)} onKeyDown={e=>{if(e.key==='Escape'&&value&&!e.nativeEvent.isComposing){e.preventDefault();e.stopPropagation();clear()}}}/>
    <InputGroupAddon align="inline-end">{value&&<InputGroupButton size="icon-sm" disabled={disabled} aria-label={`清空${label}`} title="清空搜索" onClick={clear}><X size={14}/></InputGroupButton>}{children}</InputGroupAddon>
  </InputGroup>
}

export function WorkspaceTabs({value,onChange,label,items,disabled=false}:{value:string;onChange:(value:string)=>void;label:string;items:{value:string;label:string;count?:number}[];disabled?:boolean}) {
  return <MotionConfig reducedMotion="always"><Tabs instant value={value} onValueChange={onChange} variant="underline" className="ws-tabs"><TabsList aria-label={label}>{items.map(item=><TabsTrigger key={item.value} value={item.value} disabled={disabled}>{item.label}{item.count!==undefined&&<span className="ws-count">{item.count}</span>}</TabsTrigger>)}</TabsList></Tabs></MotionConfig>
}

const wideQuery='(min-width: 1200px)'
function subscribeWide(callback:()=>void){const media=window.matchMedia(wideQuery);media.addEventListener('change',callback);return()=>media.removeEventListener('change',callback)}
export function useWideWorkspace(){return useSyncExternalStore(subscribeWide,()=>window.matchMedia(wideQuery).matches,()=>false)}

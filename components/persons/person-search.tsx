'use client'
// Adapted from ReUI's MIT search composition; filtering and sorting use the existing person list.
// https://reui.io/components/input-group/c-input-group-37
import { useRef } from 'react'
import { Search, X } from 'lucide-react'
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '@/components/ui/input-group'
import type { PersonSort } from '@/lib/person-list-model'
import { PersonSortMenu } from './person-sort'

export function PersonSearch({value,onChange,sort,onSortChange,theme}:{value:string;onChange:(value:string)=>void;sort:PersonSort;onSortChange:(sort:PersonSort)=>void;theme:'light'|'dark'}){
  const input=useRef<HTMLInputElement>(null)
  const clear=()=>{onChange('');input.current?.focus()}
  return <div className="pw-people-search-row"><InputGroup className="pw-people-search-group" aria-label="人物搜索与排序">
    <InputGroupAddon align="inline-start"><Search size={15} aria-hidden="true"/></InputGroupAddon>
    <InputGroupInput ref={input} type="search" aria-label="搜索人物" placeholder="搜索人物…" autoComplete="off" value={value} onChange={event=>onChange(event.target.value)} onKeyDown={event=>{if(event.key==='Escape'&&value&&!event.nativeEvent.isComposing){event.preventDefault();clear()}}}/>
    <InputGroupAddon align="inline-end">
      {value&&<InputGroupButton size="icon-sm" className="pw-group-action" aria-label="清空人物搜索" title="清空搜索" onClick={clear}><X size={14}/></InputGroupButton>}
      <PersonSortMenu value={sort} onChange={onSortChange} theme={theme} embedded/>
    </InputGroupAddon>
  </InputGroup></div>
}

'use client'
import * as Menu from '@radix-ui/react-dropdown-menu'
import { ArrowDown, ArrowDownWideNarrow, ArrowUp, ArrowUpNarrowWide, Check, Clock3, Film, HardDrive, ALargeSmall } from 'lucide-react'
import type { PersonSort } from '@/lib/person-list-model'
import { InputGroupButton } from '@/components/ui/input-group'
import { WorkbenchButton as Button } from './ui'
import { useWorkbenchMotion } from './motion'

const fields=[{value:'recent',label:'最近使用',Icon:Clock3},{value:'name',label:'名称',Icon:ALargeSmall},{value:'videos',label:'视频数',Icon:Film},{value:'bytes',label:'体积',Icon:HardDrive}] as const
export function PersonSortMenu({value,onChange,theme,embedded=false}:{value:PersonSort;onChange:(value:PersonSort)=>void;embedded?:boolean;theme:'light'|'dark'}){
  const {mode}=useWorkbenchMotion(),field=value.split('-')[0],ascending=value.endsWith('asc')
  const label=fields.find(item=>item.value===field)!.label,SortIcon=ascending?ArrowUpNarrowWide:ArrowDownWideNarrow
  const triggerProps={'aria-label':'人物排序',title:`排序：${label}${field==='recent'?'':ascending?'，升序':'，降序'}`}
  return <Menu.Root><Menu.Trigger asChild>{embedded?<InputGroupButton size="icon-sm" className="pw-group-action" {...triggerProps}><SortIcon size={16}/></InputGroupButton>:<Button className="pw-icon pw-sort-trigger" {...triggerProps}><SortIcon size={17}/></Button>}</Menu.Trigger><Menu.Portal><Menu.Content className="pw-sort-menu" data-theme={theme} data-motion={mode} align="end" sideOffset={6} collisionPadding={12} aria-label="人物排序选项">
    <Menu.RadioGroup value={field} onValueChange={next=>onChange(next==='recent'?'recent':`${next}-${field==='recent'?(next==='name'?'asc':'desc'):ascending?'asc':'desc'}` as PersonSort)}>
      {fields.map(({value:option,label,Icon})=><Menu.RadioItem key={option} value={option} className="pw-sort-item" onSelect={event=>{if(option!=='recent')event.preventDefault()}}><Icon size={15}/><span>{label}</span><Menu.ItemIndicator><Check size={14}/></Menu.ItemIndicator></Menu.RadioItem>)}
    </Menu.RadioGroup>
    {field!=='recent'&&<><Menu.Separator className="pw-sort-separator"/><Menu.RadioGroup value={ascending?'asc':'desc'} onValueChange={direction=>onChange(`${field}-${direction}` as PersonSort)}>{[{value:'asc',label:'升序',Icon:ArrowUp},{value:'desc',label:'降序',Icon:ArrowDown}].map(({value:direction,label,Icon})=><Menu.RadioItem key={direction} value={direction} className="pw-sort-item"><Icon size={15}/><span>{label}</span><Menu.ItemIndicator><Check size={14}/></Menu.ItemIndicator></Menu.RadioItem>)}</Menu.RadioGroup></>}
  </Menu.Content></Menu.Portal></Menu.Root>
}

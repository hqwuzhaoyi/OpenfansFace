'use client'

// Thin compositions of the existing BEUI controls and ReUI/Radix primitives.
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from 'react'
import { Collapsible } from 'radix-ui'
import { ChevronDown, Copy, AlertCircle } from 'lucide-react'
import { workspaceToast as toast } from '@/components/workbench/feedback'
import { Button as BeuiButton } from '@/components/beui/motion/button/base'
import { Card } from '@/components/beui/motion/card'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { PrivateVideo } from '@/components/private-media'
import './workspace.css'

type ButtonProps=Omit<ComponentProps<typeof BeuiButton>,'variant'|'size'>&{variant?:'default'|'primary'|'secondary'|'ghost'|'outline';size?:'sm'|'default'|'icon'|'lg'}
export function WorkspaceButton({variant='default',size='sm',className='',onPointerDown,onKeyDown,...props}:ButtonProps){
  return <BeuiButton {...props} variant={variant==='default'?'primary':variant} size={size==='icon'?'icon':size==='lg'?'md':'sm'} whileTap={undefined} ripple={false} data-slot="button" data-ui="beui-button" className={`ws-control ${size==='icon'?'ws-icon':''} ${className}`} onPointerDown={event=>{event.currentTarget.dataset.input='pointer';onPointerDown?.(event)}} onKeyDown={event=>{event.currentTarget.dataset.input='keyboard';onKeyDown?.(event)}}/>
}
export function WorkspaceCard({className='',...props}:ComponentProps<typeof Card>){return <Card {...props} size="compact" className={`ws-inner-card ${className}`}/>}
export function WorkspaceInput({className='',...props}:ComponentProps<typeof Input>){return <Input {...props} className={`ws-field ${className}`}/>}
export function WorkspaceTextarea({className='',...props}:ComponentProps<typeof Textarea>){return <Textarea {...props} className={`ws-field ws-textarea ${className}`}/>}

export function WorkspaceSelect({label,value,onChange,options,disabled=false,placeholder='请选择'}:{label:string;value:string;onChange:(value:string)=>void;options:{value:string;label:string;disabled?:boolean}[];disabled?:boolean;placeholder?:string}){
  const selected=options.some(option=>option.value===value)
  return <Select value={selected?'value:'+value:''} onValueChange={next=>onChange(next.slice(6))} disabled={disabled}>
    <SelectTrigger size="sm" className="ws-select-trigger" aria-label={label}><SelectValue placeholder={placeholder}/></SelectTrigger>
    <SelectContent className="ws-select-content" position="popper" sideOffset={5}>{options.map(option=><SelectItem key={option.value} className="ws-select-item" value={'value:'+option.value} disabled={option.disabled}>{option.label}</SelectItem>)}</SelectContent>
  </Select>
}

export function WorkspaceSection({title,count,defaultOpen=true,children}:{title:string;count?:number;defaultOpen?:boolean;children:ReactNode}){
  const [open,setOpen]=useState(defaultOpen)
  return <Collapsible.Root open={open} onOpenChange={setOpen} className="ws-section"><Collapsible.Trigger className="ws-section-trigger"><span>{title}</span>{count!==undefined&&<span className="ws-count">{count}</span>}<ChevronDown size={14} className="ws-section-chevron"/></Collapsible.Trigger><Collapsible.Content className="ws-section-content" inert={!open}>{children}</Collapsible.Content></Collapsible.Root>
}

export function WorkspaceField({label,children}:{label:string;children:ReactNode}){return <label className="ws-field-label"><span>{label}</span>{children}</label>}
export function WorkspaceMessage({children,error=false}:{children:ReactNode;error?:boolean}){return <p className={`ws-inline-message ${error?'ws-inline-error':''}`} role={error?'alert':'status'}>{error&&<AlertCircle size={14}/>}<span>{children}</span></p>}

export function WorkspacePath({value,label='完整路径'}:{value:string;label?:string}){
  const [copied,setCopied]=useState(false),timer=useRef<ReturnType<typeof setTimeout>|null>(null)
  useEffect(()=>()=>{if(timer.current)clearTimeout(timer.current)},[])
  useEffect(()=>setCopied(false),[value])
  const copy=async()=>{
    try{
      if(navigator.clipboard?.writeText)await navigator.clipboard.writeText(value)
      else{const previous=document.activeElement,node=document.createElement('textarea');node.value=value;node.readOnly=true;node.tabIndex=-1;node.style.cssText='position:fixed;opacity:0';document.body.appendChild(node);node.select();const ok=document.execCommand('copy');node.remove();if(previous instanceof HTMLElement)previous.focus({preventScroll:true});if(!ok)throw Error('copy')}
      setCopied(true);if(timer.current)clearTimeout(timer.current);timer.current=setTimeout(()=>setCopied(false),1800)
    }catch{toast.error('复制失败，可直接选择路径文字复制')}
  }
  return <div className="ws-path"><div><span>{label}</span><WorkspaceButton variant="ghost" size="icon" aria-label={`复制${label}`} title={copied?'已复制':'复制路径'} onClick={copy}><Copy size={13}/></WorkspaceButton><span role="status">{copied?'已复制':''}</span></div><code>{value}</code></div>
}

const mediaPositions=new Map<string,number>()
export function WorkspaceMedia({path,name,autoPlay=false}:{path:string;name:string;autoPlay?:boolean}){
  const [failed,setFailed]=useState(false),[attempt,setAttempt]=useState(0)
  useEffect(()=>{setFailed(false);setAttempt(0)},[path])
  return <div className="ws-media"><PrivateVideo key={`${path}:${attempt}`} src={`/api/video?p=${encodeURIComponent(path)}`} aria-label={`播放 ${name}`} controls autoPlay={autoPlay} playsInline preload="metadata" className="ws-player" onError={()=>setFailed(true)} onTimeUpdate={e=>{mediaPositions.set(path,e.currentTarget.currentTime);if(mediaPositions.size>200)mediaPositions.delete(mediaPositions.keys().next().value!)}} onLoadedMetadata={e=>{const player=e.currentTarget,position=mediaPositions.get(path)||0;if(position>0&&position<player.duration-.25)player.currentTime=position}}/>{failed&&<WorkspaceCard><WorkspaceMessage error>暂时无法播放，请核对文件路径、网络或编码支持。</WorkspaceMessage><WorkspaceButton variant="outline" onClick={()=>{setFailed(false);setAttempt(n=>n+1)}}>重新加载视频</WorkspaceButton></WorkspaceCard>}</div>
}

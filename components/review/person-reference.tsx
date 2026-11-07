'use client'
import { useRef,useState } from 'react'
import { Eye } from 'lucide-react'
import { ResponsiveDialogContent } from './responsive-dialog'
import { ReferencePanel } from './reference-panel'
import { WorkspaceButton as Button } from '@/components/workbench/inner'
import { Dialog,DialogDescription,DialogTitle } from '@/components/ui/dialog'

export function PersonReference({name,compact=false}:{name:string;compact?:boolean}){
  const [open,setOpen]=useState(false),[instant,setInstant]=useState(false),trigger=useRef<HTMLButtonElement>(null)
  return <><Button ref={trigger} variant={compact?'ghost':'outline'} aria-label={`看 ${name} 的对照视频`} title="查看对照视频" onClick={()=>{setInstant(!!document.activeElement?.matches(':focus-visible')||matchMedia('(prefers-reduced-motion: reduce)').matches);setOpen(true)}}><Eye size={14}/>{compact?'看对照':`${name} 的对照视频`}</Button><Dialog open={open} onOpenChange={setOpen}><ResponsiveDialogContent open={open} className="ws-detail-sheet" inert={!open} style={{animationDuration:instant?'0ms':'180ms'}} overlayProps={{style:{animationDuration:instant?'0ms':'140ms'}}} onCloseAutoFocus={e=>{e.preventDefault();trigger.current?.focus()}}><div className="ws-detail-heading"><DialogTitle>{name} · 对照视频</DialogTitle><DialogDescription className="sr-only">来自已归类目录，关闭后返回原位置。</DialogDescription></div><div className="ws-detail-content" data-sheet-scroll><ReferencePanel name={name}/></div><div className="ws-detail-actions"><Button variant="outline" onClick={()=>setOpen(false)}>返回</Button></div></ResponsiveDialogContent></Dialog></>
}

'use client'
import { useEffect, useRef, type ComponentProps } from 'react'
import { Button, type ButtonProps } from '@/components/beui/motion/button/base'
import { Checkbox, type CheckboxProps } from '@/components/beui/motion/checkbox'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { DialogContent } from '@/components/ui/dialog'
import { EASE_OUT_CSS } from '@/lib/beui/ease'
import { useWorkbenchMotion } from './motion'

// Scoped BEUI adapters share the application's components and workbench tokens.
export function WorkbenchButton({className='',type='button',variant='ghost',size='sm',...props}:ButtonProps) {
  const {mode}=useWorkbenchMotion()
  return <Button {...props} type={type} variant={variant} size={size} ripple={false} whileTap={undefined} data-motion={mode} data-ui="beui-button" className={`pw-control ${className}`}/>
}
export function WorkbenchCheckbox({className='',...props}:CheckboxProps){
  const {instant,mode}=useWorkbenchMotion()
  return <span className="pw-checkbox-wrap" data-motion={mode}><Checkbox {...props} motionPreset={instant?'instant':'subtle'} className={`pw-checkbox ${className}`}/></span>
}
export function WorkbenchCount({value}:{value:number}){
  const {instant}=useWorkbenchMotion()
  return <DigitSwap value={value} duration={instant?0:.16} stagger={0}/>
}
export function WorkbenchInput({className='',...props}:ComponentProps<typeof Input>) {
  return <Input {...props} className={`pw-field ${className}`}/>
}
export function WorkbenchBadge({className='',...props}:ComponentProps<typeof Badge>) {
  return <Badge {...props} variant="outline" className={`pw-badge ${className}`}/>
}
export function WorkbenchDialogContent({onCloseAutoFocus,onOpenAutoFocus,...props}:ComponentProps<typeof DialogContent>){
  const {instant,mode}=useWorkbenchMotion()
  const returnFocus=useRef<HTMLElement|null>(null),content=useRef<HTMLDivElement>(null),entrance=useRef<Animation[]>([])
  useEffect(()=>{if(instant){entrance.current.forEach(animation=>animation.cancel());entrance.current=[]}},[instant])
  useEffect(()=>()=>entrance.current.forEach(animation=>animation.cancel()),[])
  return <DialogContent {...props} ref={content} data-motion={mode} data-pw-entry="managed" overlayProps={{className:'pw-dialog-overlay',style:{animationDuration:instant?'0ms':'180ms',animationTimingFunction:EASE_OUT_CSS}}} onOpenAutoFocus={event=>{
    returnFocus.current=document.activeElement as HTMLElement
    if(!instant&&content.current){
      const options={duration:180,easing:EASE_OUT_CSS}
      entrance.current.push(content.current.animate([{opacity:0,transform:'scale(.98)'},{opacity:1,transform:'scale(1)'}],options))
      const overlay=content.current.previousElementSibling
      if(overlay instanceof HTMLElement&&overlay.classList.contains('pw-dialog-overlay'))entrance.current.push(overlay.animate([{opacity:0},{opacity:1}],options))
    }
    onOpenAutoFocus?.(event)
  }} onCloseAutoFocus={event=>{
    onCloseAutoFocus?.(event)
    if(!event.defaultPrevented&&returnFocus.current?.isConnected){event.preventDefault();returnFocus.current.focus()}
  }}/>
}

'use client'
import { useEffect, useState } from 'react'
import { DialogContent } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'

/** Keep an iPhone sheet inside the visual viewport when Safari opens its keyboard. */
export function ResponsiveDialogContent({open,className,onEscapeKeyDown,...props}:React.ComponentProps<typeof DialogContent>&{open:boolean}) {
  const [node,setNode]=useState<HTMLDivElement|null>(null)
  useEffect(()=>{
    if(!open||!node)return
    let frame=0,scrollFrame=0,baseline=window.innerHeight
    const update=()=>{
      const viewport=window.visualViewport,height=Math.round(viewport?.height||window.innerHeight)
      const focused=document.activeElement
      const typing=focused instanceof HTMLElement&&node.contains(focused)&&focused.matches('input,textarea,[contenteditable="true"]')
      if(!typing&&height>baseline*.8)baseline=Math.max(height,window.innerHeight)
      const keyboard=typing&&baseline-height>140
      node.style.setProperty('--sheet-height',`${height}px`)
      node.style.setProperty('--sheet-top',`${Math.round(viewport?.offsetTop||0)}px`)
      node.dataset.keyboard=String(keyboard)
      if(keyboard&&focused instanceof HTMLElement&&matchMedia('(max-width:1023px)').matches){
        cancelAnimationFrame(scrollFrame)
        scrollFrame=requestAnimationFrame(()=>{
          const area=node.querySelector('[data-sheet-scroll]')?.getBoundingClientRect(),rect=focused.getBoundingClientRect()
          if(area&&(rect.bottom>area.bottom||rect.top<area.top))focused.scrollIntoView({block:'nearest',inline:'nearest'})
        })
      }
    }
    const schedule=()=>{cancelAnimationFrame(frame);frame=requestAnimationFrame(update)}
    const viewport=window.visualViewport
    viewport?.addEventListener('resize',schedule);viewport?.addEventListener('scroll',schedule)
    window.addEventListener('resize',schedule);document.addEventListener('focusin',schedule);document.addEventListener('focusout',schedule)
    schedule()
    return()=>{cancelAnimationFrame(frame);cancelAnimationFrame(scrollFrame);viewport?.removeEventListener('resize',schedule);viewport?.removeEventListener('scroll',schedule);window.removeEventListener('resize',schedule);document.removeEventListener('focusin',schedule);document.removeEventListener('focusout',schedule)}
  },[open,node])
  return <DialogContent {...props} onEscapeKeyDown={event=>{if(event.isComposing||event.keyCode===229||(event.target instanceof HTMLInputElement&&event.target.type==='search'&&!!event.target.value&&!!event.target.closest('[data-slot=input-group]'))){event.preventDefault();return}onEscapeKeyDown?.(event)}} ref={setNode} className={cn('mobile-review-sheet top-0 left-0 translate-x-0 translate-y-0 lg:top-1/2 lg:left-1/2 lg:-translate-x-1/2 lg:-translate-y-1/2',className)}/>
}

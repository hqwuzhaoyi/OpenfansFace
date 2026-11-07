'use client'
import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'
import { MotionConfig, motion, useIsPresent, useReducedMotion } from 'motion/react'
import { EASE_OUT } from '@/lib/beui/ease'

const InteractionContext=createContext(false)
/** Portalled dialogs share the input modality of the workbench. */
export function WorkbenchMotion({children}:{children:ReactNode}){
  const [keyboard,setKeyboard]=useState(false)
  useEffect(()=>{
    const key=()=>setKeyboard(true),pointer=()=>setKeyboard(false)
    document.addEventListener('keydown',key,true);document.addEventListener('pointerdown',pointer,true)
    return()=>{document.removeEventListener('keydown',key,true);document.removeEventListener('pointerdown',pointer,true)}
  },[])
  return <InteractionContext.Provider value={keyboard}><MotionConfig reducedMotion="user">{children}</MotionConfig></InteractionContext.Provider>
}
export function useWorkbenchMotion(){
  const keyboard=useContext(InteractionContext),reduce=useReducedMotion()
  return {instant:keyboard||!!reduce,mode:keyboard?'keyboard':reduce?'reduced':'pointer'}
}
export function WorkbenchStep({children,phase,className}:{children:ReactNode;phase:string;className?:string}){
  const {instant}=useWorkbenchMotion()
  return <motion.div key={phase} className={className} initial={instant?false:{opacity:0,transform:'translateY(4px)'}} animate={{opacity:1,transform:'translateY(0px)'}} transition={{duration:instant?0:.16,ease:EASE_OUT}}>{children}</motion.div>
}
export function WorkbenchNotice({children}:{children:ReactNode}){
  const {instant}=useWorkbenchMotion(),present=useIsPresent()
  return <motion.div className="pw-notice" role="status" inert={!present} aria-hidden={!present||undefined} style={{pointerEvents:present?'auto':'none'}} initial={instant?false:{opacity:0,transform:'translate(-50%,8px)'}} animate={{opacity:1,transform:'translate(-50%,0px)'}} exit={{opacity:0,transform:instant?'translate(-50%,0px)':'translate(-50%,8px)'}} transition={{duration:instant?0:.18,ease:EASE_OUT}}>{children}</motion.div>
}

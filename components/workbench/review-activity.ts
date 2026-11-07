'use client'
import {useCallback,useEffect,useRef} from 'react'
import {canAutoRefreshReview} from '@/lib/review-continuity-model'

export function useReviewActivity(protectedWork:boolean){
  const activity=useRef({at:Date.now(),version:0}),protectedRef=useRef(protectedWork)
  protectedRef.current=protectedWork
  useEffect(()=>{
    const touch=(event?:Event)=>{if(event?.type==='pointermove'&&Date.now()-activity.current.at<500)return;activity.current={at:Date.now(),version:activity.current.version+1}}
    const visible=()=>{if(document.visibilityState==='visible')touch()}
    const events=['pointerdown','pointermove','keydown','input','wheel','touchstart','scroll']
    for(const name of events)document.addEventListener(name,touch,{capture:true,passive:true})
    window.addEventListener('focus',touch);document.addEventListener('visibilitychange',visible)
    return()=>{for(const name of events)document.removeEventListener(name,touch,true);window.removeEventListener('focus',touch);document.removeEventListener('visibilitychange',visible)}
  },[])
  const version=useCallback(()=>activity.current.version,[])
  const canApply=useCallback((requestActivity=activity.current.version)=>canAutoRefreshReview({now:Date.now(),lastActivity:activity.current.at,protectedWork:document.visibilityState!=='visible'||!!document.querySelector('[role="dialog"][data-state="open"],[role="alertdialog"][data-state="open"]')||protectedRef.current||[...document.querySelectorAll('video')].some(v=>!v.paused&&!v.ended),requestActivity,currentActivity:activity.current.version}),[])
  return {version,canApply}
}

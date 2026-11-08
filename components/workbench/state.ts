'use client'
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { readWorkspaceQuery,writeWorkspaceQuery } from '@/lib/workspace-query'

/** URL state is shared by both workbenches; Back/Forward restores every filter. */
export function useWorkspaceQuery<T extends Record<string,string>>(defaults:T,allowed:Partial<Record<keyof T,readonly string[]>>={}) {
  const config=useRef({defaults,allowed}),[values,setValues]=useState<T>(defaults),[ready,setReady]=useState(false)
  useEffect(()=>{const restore=()=>{setValues(readWorkspaceQuery(new URLSearchParams(window.location.search),config.current.defaults,config.current.allowed));setReady(true)};restore();window.addEventListener('popstate',restore);return()=>window.removeEventListener('popstate',restore)},[])
  useEffect(()=>{if(!ready)return;const url=new URL(window.location.href);url.search=writeWorkspaceQuery(url.searchParams,values,config.current.defaults).toString();if(url.href!==window.location.href)window.history.replaceState(window.history.state,'',url.pathname+url.search+url.hash)},[ready,values])
  const update=useCallback((patch:Partial<T>)=>setValues(previous=>readWorkspaceQuery(new URLSearchParams({...previous,...patch}),config.current.defaults,config.current.allowed)),[])
  return {values,ready,update}
}

const scrollPositions=new Map<string,number>()
export function useWorkspaceScroll(key:string,ready:boolean){
  const ref=useRef<HTMLDivElement>(null)
  useLayoutEffect(()=>{if(ready&&ref.current)ref.current.scrollTop=scrollPositions.get(key)||0},[key,ready])
  const onScroll=useCallback((event:React.UIEvent<HTMLDivElement>)=>{if(!ready)return;scrollPositions.set(key,event.currentTarget.scrollTop);if(scrollPositions.size>100)scrollPositions.delete(scrollPositions.keys().next().value!)},[key,ready])
  return {ref,onScroll}
}

const workbenchCache=new Map<string,unknown>()
export function readWorkbenchCache<T>(key:string):T|undefined{
  return workbenchCache.get(key) as T|undefined
}
export function writeWorkbenchCache<T>(key:string,value:T):T{
  if(!workbenchCache.has(key)&&workbenchCache.size>=200){
    const oldest=workbenchCache.keys().next().value
    if(oldest!==undefined)workbenchCache.delete(oldest)
  }
  workbenchCache.set(key,value)
  return value
}
export function deleteWorkbenchCache(prefix:string){
  for(const key of workbenchCache.keys()){
    if(key===prefix||key.startsWith(prefix))workbenchCache.delete(key)
  }
}

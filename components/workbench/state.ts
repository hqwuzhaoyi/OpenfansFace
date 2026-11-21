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

type CacheEntry<T = unknown> = { value: T; at: number }
const SESSION_PREFIX = 'wb-cache:'
const workbenchCache=new Map<string,CacheEntry>()
const persistedKeys:string[]=[]

function evictPersistedCache(limit=16){
  while(persistedKeys.length>limit){
    const oldest=persistedKeys.shift()
    if(oldest)try{window.sessionStorage.removeItem(SESSION_PREFIX+oldest)}catch{}
  }
}

export function readWorkbenchCache<T>(key:string,maxAgeMs?:number):T|undefined{
  const now=Date.now()
  const entry=workbenchCache.get(key)
  if(entry){
    if(maxAgeMs===undefined||now-entry.at<=maxAgeMs)return entry.value as T
    workbenchCache.delete(key)
  }
  return undefined
}

export function readPersistedWorkbenchCache<T>(key:string,maxAgeMs?:number):T|undefined{
  const hit=readWorkbenchCache<T>(key,maxAgeMs)
  if(hit!==undefined)return hit
  if(typeof window==='undefined')return undefined
  const now=Date.now()
  try{
    const raw=window.sessionStorage.getItem(SESSION_PREFIX+key)
    if(!raw)return undefined
    const parsed=JSON.parse(raw) as CacheEntry<T>
    if(typeof parsed?.at!=='number'||(maxAgeMs!==undefined&&now-parsed.at>maxAgeMs)){
      window.sessionStorage.removeItem(SESSION_PREFIX+key)
      return undefined
    }
    workbenchCache.set(key,parsed)
    return parsed.value
  }catch{
    return undefined
  }
}

export function writeWorkbenchCache<T>(key:string,value:T,options?:{persist?:boolean}):T{
  const entry:CacheEntry<T>={value,at:Date.now()}
  if(!workbenchCache.has(key)&&workbenchCache.size>=200){
    const oldest=workbenchCache.keys().next().value
    if(oldest!==undefined)workbenchCache.delete(oldest)
  }
  workbenchCache.set(key,entry)
  if(options?.persist&&typeof window!=='undefined'){
    const serialized=JSON.stringify(entry)
    try{
      window.sessionStorage.setItem(SESSION_PREFIX+key,serialized)
    }catch{
      evictPersistedCache(Math.max(0,Math.floor(persistedKeys.length/2)))
      try{window.sessionStorage.setItem(SESSION_PREFIX+key,serialized)}catch{}
    }
    const existingIdx=persistedKeys.indexOf(key)
    if(existingIdx>=0)persistedKeys.splice(existingIdx,1)
    persistedKeys.push(key)
    evictPersistedCache(16)
  }
  return value
}

export function deletePersistedWorkbenchCache(prefix:string){
  if(typeof window==='undefined')return
  try{
    for(let i=window.sessionStorage.length-1;i>=0;i--){
      const storageKey=window.sessionStorage.key(i)
      if(!storageKey?.startsWith(SESSION_PREFIX))continue
      const cacheKey=storageKey.slice(SESSION_PREFIX.length)
      if(cacheKey===prefix||cacheKey.startsWith(prefix)){
        window.sessionStorage.removeItem(storageKey)
        const idx=persistedKeys.indexOf(cacheKey)
        if(idx>=0)persistedKeys.splice(idx,1)
      }
    }
  }catch{}
}

export function deleteWorkbenchCache(prefix:string){
  for(const key of workbenchCache.keys()){
    if(key===prefix||key.startsWith(prefix))workbenchCache.delete(key)
  }
  deletePersistedWorkbenchCache(prefix)
}

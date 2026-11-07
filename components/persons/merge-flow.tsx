'use client'
import { useLayoutEffect, useRef, useState } from 'react'
import { motion } from 'motion/react'
import { ArrowRight, Check, LoaderCircle } from 'lucide-react'
import { EASE_OUT } from '@/lib/beui/ease'
import type { PersonBatchProgress, PersonBatchResult } from '@/lib/person-batch-model'
import { WorkbenchCount as Count } from './ui'
import { useWorkbenchMotion } from './motion'

interface Flight {name:string;fromX:number;fromY:number;toX:number;toY:number}
export function MergeFlow({people,target,applying,progress,result,totalVideos}:{people:string[];target:string;applying:boolean;progress:PersonBatchProgress|null;result:PersonBatchResult|null;totalVideos:number|null}){
  const {instant,mode}=useWorkbenchMotion(),root=useRef<HTMLDivElement>(null),destination=useRef<HTMLDivElement>(null),nodes=useRef(new Map<string,HTMLDivElement>())
  const previous=useRef(new Set<string>()),[flights,setFlights]=useState<Flight[]>([])
  const completed=result?result.completed.map(item=>item.person):progress?.completed||[],sources=people.filter(person=>person!==target),signature=JSON.stringify(completed)
  useLayoutEffect(()=>{
    const next=new Set<string>(JSON.parse(signature)),added=[...next].filter(name=>!previous.current.has(name));previous.current=next
    if(instant){setFlights([]);return}
    const area=root.current?.getBoundingClientRect(),to=destination.current?.getBoundingClientRect()
    if(!area||!to)return
    const fresh=added.slice(-4).flatMap(name=>{const from=nodes.current.get(name)?.getBoundingClientRect();return from?[{name,fromX:from.x-area.x,fromY:from.y-area.y,toX:to.x-area.x,toY:to.y-area.y}]:[]})
    if(fresh.length)setFlights(current=>[...current,...fresh])
  },[signature,instant])
  if(!target)return <div className="pw-merge-flow-placeholder">先选择保留人物，查看合并方向与视频。</div>
  const title=result?.ok?'合并完成':result?'合并已停止':applying?(progress?.active_person?`正在合入 ${progress.active_person}`:'正在确认合并状态…'):'合并预览'
  return <div ref={root} className="pw-merge-flow" data-complete={result?.ok||undefined}>
    <div className="pw-merge-flow-heading" role="status"><strong>{title}</strong><span>{applying||result?<><Count value={completed.length}/> / {sources.length} 人已完成</>:`${sources.length} 人 → 1 人`}</span></div>
    <div className="pw-merge-direction"><div className="pw-merge-sources">{sources.map(name=><div key={name} className="pw-merge-source" data-complete={completed.includes(name)}><div className="pw-merge-avatar" ref={node=>{if(node)nodes.current.set(name,node);else nodes.current.delete(name)}}>{name.slice(0,1)}</div><span>{name}</span>{completed.includes(name)?<Check size={14}/>:applying&&progress?.active_person===name?<LoaderCircle size={14} className="pw-busy-icon" data-motion={mode}/>:null}</div>)}</div><ArrowRight size={22} aria-hidden="true"/><div className="pw-merge-destination"><div ref={destination} className="pw-merge-avatar">{result?.ok?<Check size={18}/>:target.slice(0,1)}</div><strong>{target}</strong><small>{totalVideos===null?'视频数量待核对':`${result?.ok?'合并后':'预计合并后'} ${totalVideos} 个视频`}</small></div></div>
    {(applying||result)&&<div className="pw-merge-progress" role="progressbar" aria-label="人物合并进度" aria-valuemin={0} aria-valuemax={sources.length} aria-valuenow={completed.length}><motion.div initial={false} animate={{transform:`scaleX(${sources.length?completed.length/sources.length:0})`}} transition={{duration:instant?0:.18,ease:EASE_OUT}}/></div>}
    {flights.map(flight=><motion.div key={flight.name} className="pw-merge-avatar pw-merge-flight" aria-hidden="true" initial={{opacity:1,transform:`translate(${flight.fromX}px,${flight.fromY}px) scale(1)`}} animate={{opacity:0,transform:`translate(${flight.toX}px,${flight.toY}px) scale(.96)`}} transition={{duration:.24,ease:EASE_OUT}} onAnimationComplete={()=>setFlights(current=>current.filter(item=>item.name!==flight.name))}>{flight.name.slice(0,1)}</motion.div>)}
  </div>
}

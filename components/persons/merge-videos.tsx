'use client'
import { useCallback, useEffect, useRef, useState } from 'react'
import { FileVideo, Play, X } from 'lucide-react'
import { Card } from '@/components/beui/motion/card'
import { api } from '@/lib/face-api'
import type { PersonLibraryPreview, PersonVideo } from '@/lib/person-library-model'
import { WorkbenchButton as Button } from './ui'

export interface PersonPreviewState {data?:PersonLibraryPreview;loading?:boolean;error?:string}
export function usePersonPreviews(people:string[],enabled:boolean){
  const [states,setStates]=useState<Record<string,PersonPreviewState>>({})
  const controller=useRef<AbortController|null>(null)
  const load=useCallback(async(name:string,offset=0)=>{
    const signal=controller.current?.signal
    if(!signal||signal.aborted)return
    setStates(current=>({...current,[name]:{...current[name],loading:true,error:undefined}}))
    try{
      const data=await api<PersonLibraryPreview>(`/api/persons/library?preview=1&name=${encodeURIComponent(name)}&offset=${offset}`,{signal})
      if(signal.aborted)return
      setStates(current=>({...current,[name]:{data:{...data,files:offset?[...(current[name]?.data?.files||[]),...data.files]:data.files},loading:false}}))
    }catch(error){if(!signal.aborted)setStates(current=>({...current,[name]:{...current[name],loading:false,error:(error as Error).message}}))}
  },[])
  useEffect(()=>{
    if(!enabled)return
    const abort=new AbortController();controller.current=abort;let index=0
    const worker=async()=>{while(index<people.length&&!abort.signal.aborted)await load(people[index++])}
    void Promise.all(Array.from({length:Math.min(3,people.length)},worker))
    return()=>abort.abort()
  },[people,enabled,load])
  return {states,load}
}
const size=(bytes:number)=>bytes>=2**30?`${(bytes/2**30).toFixed(2)} GB`:`${(bytes/2**20).toFixed(1)} MB`
const videoUrl=(video:PersonVideo)=>`/api/video?p=${encodeURIComponent(video.path)}`
function VideoCover({video}:{video:PersonVideo}){
  const ref=useRef<HTMLSpanElement>(null),[visible,setVisible]=useState(false),[failed,setFailed]=useState(false)
  useEffect(()=>{
    if(!ref.current)return
    const observer=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting)){setVisible(true);observer.disconnect()}},{rootMargin:'100px'})
    observer.observe(ref.current);return()=>observer.disconnect()
  },[])
  return <span ref={ref} className="pw-merge-cover" aria-hidden="true">{failed?<FileVideo size={22}/>:video.thumb?<img src={video.thumb} alt="" loading="lazy" onError={()=>setFailed(true)}/>:visible?<video src={videoUrl(video)+'#t=0.1'} muted playsInline preload="metadata" onError={()=>setFailed(true)} tabIndex={-1}/>:<FileVideo size={22}/>}<span className="pw-merge-play"><Play size={15} fill="currentColor"/></span></span>
}
export interface MergePlaying {person:string;video:PersonVideo}
export function MergePersonVideos({name,state,target,disabled,playing,onTarget,onPlay,onLoad}:{name:string;state?:PersonPreviewState;target:string;disabled:boolean;playing:MergePlaying|null;onTarget:(name:string)=>void;onPlay:(video:MergePlaying|null)=>void;onLoad:(name:string,offset?:number)=>void}){
  const [expanded,setExpanded]=useState(false),[playError,setPlayError]=useState('')
  const player=useRef<HTMLVideoElement>(null),data=state?.data,active=playing?.person===name?playing.video:null
  useEffect(()=>{setPlayError('');if(active)player.current?.scrollIntoView({block:'nearest'})},[active])
  const files=expanded?data?.files||[]:(data?.files||[]).slice(0,2)
  return <Card size="compact" selected={target===name} className="pw-merge-person-card" data-person={name}>
    <header><div><strong title={name}>{name}</strong><small>{data?`${data.total_videos} 个视频 · ${size(data.total_bytes)}`:state?.error?'读取失败':'读取视频…'}</small></div><Button aria-label={`保留人物 ${name}`} aria-pressed={target===name} disabled={disabled} onClick={()=>onTarget(name)} className={target===name?'pw-primary':'pw-outline'}>{target===name?'保留此人':'设为保留'}</Button></header>
    {active&&<div className="pw-merge-player"><div><span>{active.name}</span><Button aria-label="关闭视频预览" onClick={()=>onPlay(null)}><X size={15}/></Button></div><video ref={player} key={active.path} src={videoUrl(active)} poster={active.thumb||undefined} controls autoPlay playsInline onError={()=>setPlayError('暂时无法播放，请检查文件或浏览器编码支持。')}/>{playError&&<p role="alert">{playError}</p>}<code>{active.path}</code></div>}
    <div className="pw-merge-video-grid">{files.map(video=><Button key={video.path} className="pw-merge-video" aria-label={`预览 ${name} 的 ${video.name}`} disabled={disabled} onClick={()=>onPlay({person:name,video})}><VideoCover video={video}/><strong title={video.name}>{video.name}</strong><small>{video.folder==='.'?'根目录':video.folder} · {size(video.size)}</small></Button>)}</div>
    {data&&!data.total_videos&&<p className="pw-merge-no-video">暂无视频，仍可合并人物资料。</p>}
    {state?.error&&<div className="pw-error" role="alert"><p>{state.error}</p><Button disabled={state.loading||disabled} onClick={()=>onLoad(name,data?.files.length||0)}>重新加载视频</Button></div>}
    {data&&data.total_videos>files.length&&<Button className="pw-merge-more" disabled={state?.loading||disabled} onClick={()=>{if(!expanded){setExpanded(true);return}if(data.next_offset!==null)onLoad(name,data.next_offset)}}>{state?.loading?'读取中…':`查看更多视频（${files.length} / ${data.total_videos}）`}</Button>}
    {expanded&&<Button className="pw-merge-more" disabled={disabled} onClick={()=>setExpanded(false)}>收起视频</Button>}
  </Card>
}

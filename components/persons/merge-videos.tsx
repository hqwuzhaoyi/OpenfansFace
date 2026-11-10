'use client'
import { ExternalPlayerBar, WorkbenchVideoPlayer } from '@/components/workbench/video-player'
import { VideoCover } from '@/components/review/video-cover'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, Copy, Folder, FolderCheck, FolderInput, X } from 'lucide-react'
import { ActionSwapIcon, ActionSwapText } from '@/components/beui/motion/action-swap'
import { Avatar } from '@/components/beui/motion/avatar-group'
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
export interface MergePlaying {person:string;video:PersonVideo}
export function MergePersonVideos({
  name,state,repThumb,target,disabled,playing,onTarget,onPlay,onLoad
}:{
  name:string;state?:PersonPreviewState;repThumb?:string|null;target:string;disabled:boolean;playing:MergePlaying|null
  onTarget:(name:string)=>void;onPlay:(video:MergePlaying|null)=>void;onLoad:(name:string,offset?:number)=>void
}){
  const [expanded,setExpanded]=useState(false),[playError,setPlayError]=useState(''),[copied,setCopied]=useState(false)
  const player=useRef<HTMLVideoElement>(null),data=state?.data,active=playing?.person===name?playing.video:null
  useEffect(()=>{setPlayError('');setCopied(false);if(active)player.current?.scrollIntoView({block:'nearest'})},[active])
  useEffect(()=>{
    if(!active)return
    const onKey=(e:KeyboardEvent)=>{
      if(e.defaultPrevented||e.metaKey||e.ctrlKey||e.altKey)return
      const tag=(e.target as HTMLElement|null)?.tagName
      if(tag&&['INPUT','TEXTAREA','SELECT'].includes(tag))return
      if(e.key==='c'||e.key==='C'){e.preventDefault();void navigator.clipboard.writeText(active.path).then(()=>{setCopied(true);setTimeout(()=>setCopied(false),1400)}).catch(()=>{})}
    }
    window.addEventListener('keydown',onKey)
    return()=>window.removeEventListener('keydown',onKey)
  },[active])
  const files=expanded?data?.files||[]:(data?.files||[]).slice(0,2)
  const isTarget=target===name,thumb=data?.files.find(f=>f.thumb)?.thumb||repThumb||undefined
  const roleState=isTarget?'kept':target?'source':'idle'
  return <Card size="compact" selected={isTarget} className="pw-merge-person-card" data-person={name} data-role={roleState}>
    <header>
      <div className="pw-merge-person-title">
        <Avatar name={name} src={thumb} size="sm" className="pw-merge-card-avatar"/>
        <div>
          <strong title={name}>{name}</strong>
          <small>{data?`${data.total_videos} 个视频 · ${size(data.total_bytes)}`:state?.error?'读取失败':'读取视频…'}</small>
        </div>
      </div>
      <Button aria-label={`保留人物 ${name}`} aria-pressed={isTarget} disabled={disabled} onClick={()=>onTarget(name)} className={isTarget?'pw-primary':'pw-outline'}>
        <ActionSwapIcon value={isTarget?'kept':'pick'} animation="roll">
          {isTarget?<Check size={13}/>:<FolderCheck size={13}/>}
        </ActionSwapIcon>
        <ActionSwapText value={isTarget?'kept':'pick'} animation="roll">
          {isTarget?'保留此人':'设为保留'}
        </ActionSwapText>
      </Button>
    </header>
    {data?.root&&<div className="pw-merge-card-folder" data-role={roleState} title={data.root}>
      {isTarget?<FolderCheck size={12}/>:target?<FolderInput size={12}/>:<Folder size={12}/>}
      <code>{data.root}</code>
      {target&&(isTarget?<b>目标保留目录</b>:<span>→ 合入 {target}</span>)}
    </div>}
    {active&&<div className="pw-merge-player"><div className="pw-merge-player-head"><span>{active.name}</span><div className="pw-merge-player-actions"><Button aria-label="复制视频路径" title="复制视频路径 (C)" onClick={()=>void navigator.clipboard.writeText(active.path).then(()=>{setCopied(true);setTimeout(()=>setCopied(false),1400)}).catch(()=>{})}><ActionSwapIcon value={copied?'copied':'copy'} animation="roll">{copied?<Check size={12}/>:<Copy size={12}/>}</ActionSwapIcon></Button><Button aria-label="关闭视频预览" onClick={()=>onPlay(null)}><X size={15}/></Button></div></div><WorkbenchVideoPlayer videoRef={player} key={active.path} src={videoUrl(active)} name={active.name} poster={active.thumb||undefined} autoPlay compact enableGlobalHotkeys onError={()=>setPlayError('暂时无法播放，请检查文件或浏览器编码支持，或使用外部播放器打开。')}/>{playError&&<div role="alert"><p>{playError}</p><ExternalPlayerBar src={videoUrl(active)} name={active.name}/></div>}<code>{active.path}</code></div>}
    <div className="pw-merge-video-grid">{files.map(video=><Button key={video.path} className="pw-merge-video" aria-label={`预览 ${name} 的 ${video.name}`} disabled={disabled} onClick={()=>onPlay({person:name,video})}><VideoCover video={video} className="pw-merge-cover"/><strong title={video.name}>{video.name}</strong><small>{video.folder==='.'?'根目录':video.folder} · {size(video.size)}</small></Button>)}</div>
    {data&&!data.total_videos&&<p className="pw-merge-no-video">暂无视频，仍可合并人物资料。</p>}
    {state?.error&&<div className="pw-error" role="alert"><p>{state.error}</p><Button disabled={state.loading||disabled} onClick={()=>onLoad(name,data?.files.length||0)}>重新加载视频</Button></div>}
    {data&&data.total_videos>files.length&&<Button className="pw-merge-more" disabled={state?.loading||disabled} onClick={()=>{if(!expanded){setExpanded(true);return}if(data.next_offset!==null)onLoad(name,data.next_offset)}}>{state?.loading?'读取中…':`查看更多视频（${files.length} / ${data.total_videos}）`}</Button>}
    {expanded&&<Button className="pw-merge-more" disabled={disabled} onClick={()=>setExpanded(false)}>收起视频</Button>}
  </Card>
}

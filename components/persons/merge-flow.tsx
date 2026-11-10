'use client'
import { useLayoutEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ArrowRight, Check, FolderCheck, FolderInput, FolderOutput, LoaderCircle } from 'lucide-react'
import { ActionSwapIcon } from '@/components/beui/motion/action-swap'
import { AnimatedBadge, type AnimatedBadgeStatus } from '@/components/beui/motion/animated-badge'
import { Avatar, AvatarGroup } from '@/components/beui/motion/avatar-group'
import { VideoCover } from '@/components/review/video-cover'
import { EASE_OUT, SPRING_LAYOUT } from '@/lib/beui/ease'
import type { PersonBatchPlan, PersonBatchProgress, PersonBatchResult } from '@/lib/person-batch-model'
import type { PersonVideo } from '@/lib/person-library-model'
import type { PersonPreviewState } from './merge-videos'
import { WorkbenchButton as Button, WorkbenchCount as Count } from './ui'
import { useWorkbenchMotion } from './motion'

interface Flight {name:string;thumb?:string;fromX:number;fromY:number;toX:number;toY:number}

function tailPath(path?:string){
  if(!path)return ''
  const parts=path.replace(/\\/g,'/').split('/').filter(Boolean)
  return parts.length>2?`…/${parts.slice(-2).join('/')}`:path
}

export function MergeFlow({
  people,target,applying,progress,result,totalVideos,plan,states,thumbs,disabled,onSelectTarget
}:{
  people:string[];target:string;applying:boolean;progress:PersonBatchProgress|null;result:PersonBatchResult|null;totalVideos:number|null
  plan?:PersonBatchPlan|null;states?:Record<string,PersonPreviewState>;thumbs?:Record<string,string|null>;disabled?:boolean;onSelectTarget?:(name:string)=>void
}){
  const {instant,mode}=useWorkbenchMotion(),root=useRef<HTMLDivElement>(null),destination=useRef<HTMLDivElement>(null),nodes=useRef(new Map<string,HTMLDivElement>())
  const previous=useRef(new Set<string>()),cachedThumbs=useRef<Record<string,string>>({}),[flights,setFlights]=useState<Flight[]>([])
  const completed=result?result.completed.map(item=>item.person):progress?.completed||[],sources=people.filter(person=>person!==target),signature=JSON.stringify(completed)

  for(const name of people){
    const found=states?.[name]?.data?.files.find(f=>f.thumb)?.thumb||thumbs?.[name]
    if(found)cachedThumbs.current[name]=found
  }
  const getThumb=(name:string)=>states?.[name]?.data?.files.find(f=>f.thumb)?.thumb||thumbs?.[name]||cachedThumbs.current[name]||undefined
  const getVideos=(name:string):PersonVideo[]=>states?.[name]?.data?.files||[]
  const getCount=(name:string)=>states?.[name]?.data?.total_videos??null
  const getSourceDir=(name:string)=>plan?.plans.find(p=>p.person===name)?.source_directory||states?.[name]?.data?.root||`creators/${name}`
  const targetDir=target?(plan?.plans[0]?.target_directory||states?.[target]?.data?.root||`creators/${target}`):''

  useLayoutEffect(()=>{
    const next=new Set<string>(JSON.parse(signature)),added=[...next].filter(name=>!previous.current.has(name));previous.current=next
    if(instant){setFlights([]);return}
    const area=root.current?.getBoundingClientRect(),to=destination.current?.getBoundingClientRect()
    if(!area||!to)return
    const fresh=added.slice(-4).flatMap(name=>{
      const from=nodes.current.get(name)?.getBoundingClientRect()
      return from?[{name,thumb:getThumb(name),fromX:from.x-area.x,fromY:from.y-area.y,toX:to.x-area.x,toY:to.y-area.y}]:[]
    })
    if(fresh.length)setFlights(current=>[...current,...fresh])
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[signature,instant])

  const members=people.map(name=>({id:name,name,src:getThumb(name)}))
  const badgeStatus:AnimatedBadgeStatus=result?.ok?'success':result?'warning':applying?'loading':target?'info':'neutral'
  const activePerson=applying?(progress?.active_person||sources.find(name=>!completed.includes(name))||''):''
  const planFiles=plan?.plans.reduce((sum,item)=>sum+item.files.length,0)??null
  const title=!target
    ?'先选保留人物'
    :result?.ok
      ?'合并完成'
      :result
        ?'合并已停止'
        :applying
          ?(activePerson?`正在合入 ${activePerson}`:'正在确认合并状态…')
          :planFiles!==null
            ?`待确认合入 · ${planFiles} 个文件`
            :`准备合入 · ${sources.length} → 1`

  if(!target){
    return <div className="pw-merge-flow pw-merge-flow-empty">
      <div className="pw-merge-flow-heading" role="status">
        <AnimatedBadge status="neutral" size="sm" contentKey="empty" className="pw-merge-status-badge">{title}</AnimatedBadge>
        <span>已选 <Count value={people.length}/> 人 · 点击人物设为保留目录</span>
      </div>
      <div className="pw-merge-pick-strip">
        <AvatarGroup members={members} max={5} size="sm" label="待合并人物头像"/>
        <div className="pw-merge-pick-list">
          {people.map(name=>{
            const count=getCount(name),dir=states?.[name]?.data?.root
            return <Button key={name} className="pw-merge-pick-chip" disabled={disabled} onClick={()=>onSelectTarget?.(name)}>
              <Avatar name={name} src={getThumb(name)} size="sm" className="pw-merge-chip-avatar"/>
              <span className="pw-merge-pick-copy">
                <strong>{name}</strong>
                <small>{count!==null?`${count} 个视频`:'读取中…'}{dir?` · ${tailPath(dir)}`:''}</small>
              </span>
            </Button>
          })}
        </div>
      </div>
    </div>
  }

  const previewClips:{person:string;video:PersonVideo}[]=[]
  const seenPaths=new Set<string>()
  for(const name of [target,...sources]){
    for(const video of getVideos(name).slice(0,2)){
      if(!seenPaths.has(video.path)&&previewClips.length<4){
        seenPaths.add(video.path)
        previewClips.push({person:name,video})
      }
    }
  }
  const targetThumb=getThumb(target),targetFirstVideo=getVideos(target)[0]

  return <div ref={root} className="pw-merge-flow" data-complete={result?.ok||undefined} data-applying={applying||undefined}>
    <div className="pw-merge-flow-heading" role="status">
      <div className="pw-merge-heading-main">
        <AnimatedBadge status={badgeStatus} size="sm" contentKey={`${badgeStatus}:${title}`} className="pw-merge-status-badge">{title}</AnimatedBadge>
        <strong>{sources.length} 个来源目录 → {target}</strong>
      </div>
      <span>{applying||result?<><Count value={completed.length}/> / {sources.length} 人已完成</>:`${sources.length} 人 → 1 人`}</span>
    </div>

    <div className="pw-merge-direction">
      <div className="pw-merge-sources">
        <AnimatePresence initial={false} mode="popLayout">
          {sources.map(name=>{
            const done=completed.includes(name),active=applying&&activePerson===name
            const thumb=getThumb(name),videos=getVideos(name),count=getCount(name),srcDir=getSourceDir(name)
            const stateKey=done?'done':active?'busy':'ready'
            return <motion.div
              key={name}
              layout={!instant}
              transition={SPRING_LAYOUT}
              className="pw-merge-source"
              data-complete={done}
              data-active={active||undefined}
            >
              <div className="pw-merge-avatar" ref={node=>{if(node)nodes.current.set(name,node);else nodes.current.delete(name)}}>
                {thumb?<img data-private-media src={thumb} alt="" draggable={false} loading="lazy"/>:videos[0]?<VideoCover video={videos[0]} className="pw-merge-avatar-cover"/>:name.slice(0,1)}
              </div>
              <div className="pw-merge-source-copy">
                <span title={name}>{name}</span>
                <small title={srcDir}><code>{tailPath(srcDir)}</code>{count!==null?` · ${count} 个视频`:''}</small>
              </div>
              {videos.length>0&&<div className="pw-merge-source-thumbs" aria-hidden="true">
                {videos.slice(0,2).map(video=><span key={video.path} className="pw-merge-mini-frame"><VideoCover video={video} className="pw-merge-mini-cover"/></span>)}
              </div>}
              <ActionSwapIcon value={stateKey} animation="roll" className="pw-merge-source-state">
                {done?<Check size={14}/>:active?<LoaderCircle size={14} className="pw-busy-icon" data-motion={mode}/>:<ArrowRight size={13} className="pw-merge-ready-icon"/>}
              </ActionSwapIcon>
            </motion.div>
          })}
        </AnimatePresence>
      </div>

      <div className="pw-merge-bridge" aria-hidden="true">
        <span className="pw-merge-bridge-track">
          {!instant&&!result&&<motion.span
            className="pw-merge-bridge-Stream"
            animate={{transform:['translateX(-12px)','translateX(12px)'],opacity:[0,1,0]}}
            transition={{duration:applying?.6:1.15,repeat:Infinity,ease:'linear'}}
          />}
        </span>
        <span className="pw-merge-bridge-node">
          <ActionSwapIcon value={result?.ok?'done':applying?'busy':'ready'} animation="roll">
            {result?.ok?<Check size={14}/>:applying?<FolderInput size={14}/>:<ArrowRight size={15}/>}
          </ActionSwapIcon>
        </span>
      </div>

      <motion.div layout={!instant} transition={SPRING_LAYOUT} className="pw-merge-destination">
        <div className="pw-merge-dest-top">
          <div ref={destination} className="pw-merge-avatar pw-merge-dest-avatar">
            {targetThumb?<img data-private-media src={targetThumb} alt="" draggable={false} loading="lazy"/>:targetFirstVideo?<VideoCover video={targetFirstVideo} className="pw-merge-avatar-cover"/>:target.slice(0,1)}
            {result?.ok&&<span className="pw-merge-dest-check" aria-hidden="true"><Check size={11}/></span>}
          </div>
          <div className="pw-merge-dest-copy">
            <div className="pw-merge-dest-title">
              <strong title={target}>{target}</strong>
              <AvatarGroup members={members} max={4} size="sm" label="合并后人物组合" className="pw-merge-dest-group"/>
            </div>
            <small>{totalVideos===null?'视频数量待核对':<>{result?.ok?'合并后':'预计合并后'} <Count value={totalVideos}/> 个视频{planFiles!==null?` · 待移入 ${planFiles} 个文件`:''}</>}</small>
          </div>
        </div>

        <div className="pw-merge-folder-bar" title={targetDir}>
          {result?.ok?<FolderCheck size={13}/>:<FolderOutput size={13}/>}
          <span>{result?.ok?'已合入目录':'合并至目录'}</span>
          <code>{targetDir}</code>
        </div>

        {previewClips.length>0&&<div className="pw-merge-dest-thumbs" aria-label="合并后视频预览">
          {previewClips.map(({person:owner,video})=><div key={video.path} className="pw-merge-dest-thumb" title={`${owner} · ${video.name}`}>
            <VideoCover video={video} className="pw-merge-dest-cover"/>
            <span>{owner===target?'保留':`来自 ${owner}`}</span>
          </div>)}
        </div>}
      </motion.div>
    </div>

    {(applying||result)&&<div className="pw-merge-progress" role="progressbar" aria-label="人物合并进度" aria-valuemin={0} aria-valuemax={sources.length} aria-valuenow={completed.length}><motion.div initial={false} animate={{transform:`scaleX(${sources.length?completed.length/sources.length:0})`}} transition={{duration:instant?0:.18,ease:EASE_OUT}}/></div>}
    {flights.map(flight=><motion.div key={flight.name} className="pw-merge-avatar pw-merge-flight" aria-hidden="true" initial={{opacity:1,transform:`translate(${flight.fromX}px,${flight.fromY}px) scale(1)`}} animate={{opacity:[1,1,0],transform:`translate(${flight.toX}px,${flight.toY}px) scale(.92)`}} transition={{duration:.24,ease:EASE_OUT}} onAnimationComplete={()=>setFlights(current=>current.filter(item=>item.name!==flight.name))}>{flight.thumb?<img data-private-media src={flight.thumb} alt="" draggable={false}/>:flight.name.slice(0,1)}</motion.div>)}
  </div>
}

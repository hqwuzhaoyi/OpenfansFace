'use client'
// Shared from the existing merge preview: visible media load lazily; now retain a
// decoded frame instead of leaving a video decoder mounted for every list row.
import { useEffect,useRef,useState } from 'react'
import { FileVideo,Play } from 'lucide-react'
import { requestVideoPoster,type VideoPoster } from '@/lib/video-poster'
import './video-cover.css'

export function VideoCover({video,sampleTime=.1,className=''}:{video:{path:string;thumb?:string|null;size?:number;modified_at?:string;mtime_ns?:string};sampleTime?:number;className?:string}){
  const ref=useRef<HTMLSpanElement>(null),[visible,setVisible]=useState(false),[badThumb,setBadThumb]=useState<string|null>(null)
  const stamp=JSON.stringify([video.size,video.mtime_ns||video.modified_at]),key=JSON.stringify([video.path,stamp,sampleTime])
  const [state,setState]=useState<{key:string;poster?:VideoPoster;error?:string}|null>(null)
  const thumb=video.thumb&&badThumb!==video.thumb?video.thumb:null,current=state?.key===key?state:null
  useEffect(()=>{const element=ref.current;if(!element)return;const observer=new IntersectionObserver(entries=>setVisible(entries.some(e=>e.isIntersecting)),{rootMargin:'120px'});observer.observe(element);return()=>observer.disconnect()},[])
  useEffect(()=>{if(!visible||thumb)return;const abort=new AbortController();requestVideoPoster(video.path,stamp,sampleTime,abort.signal).then(poster=>{if(!abort.signal.aborted)setState({key,poster})}).catch(error=>{if(!abort.signal.aborted)setState({key,error:error.message})});return()=>abort.abort()},[visible,thumb,key,video.path,stamp,sampleTime])
  const image=thumb||current?.poster?.src,time=current?.poster?.time
  return <span ref={ref} className={`video-cover ${className}`} aria-hidden="true" data-frame-ready={!!image}>
    {image?<img data-private-media draggable={false} src={image} alt="" loading="lazy" onError={()=>thumb?setBadThumb(thumb):setState({key,error:'截图暂不可用'})}/>:<span className="video-cover-placeholder"><FileVideo size={22}/><small>{current?.error?'截图暂不可用':visible?'读取截图…':'视频截图'}</small></span>}
    <span className="video-cover-play"><Play size={14}/></span>{time!==undefined&&<span className="video-cover-time">{Math.floor(time/60)}:{String(Math.floor(time%60)).padStart(2,'0')}</span>}
  </span>
}

'use client'

/** Bounded, in-memory browser frames; never plays audio or writes to the media library. */
export interface VideoPoster {src:string;time:number}
const cache=new Map<string,VideoPoster>()
const queue:{signal:AbortSignal;run:()=>Promise<void>}[]=[]
let active=0
function drain(){while(active<2&&queue.length){const job=queue.shift()!;if(job.signal.aborted)continue;active++;void job.run().finally(()=>{active--;drain()})}}
function abortError(){return new DOMException('Frame request cancelled','AbortError')}

export function requestVideoPoster(path:string,stamp:string,time:number,signal:AbortSignal):Promise<VideoPoster>{
  const key=JSON.stringify([path,stamp,time])
  if(signal.aborted)return Promise.reject(abortError())
  const cached=cache.get(key);if(cached){cache.delete(key);cache.set(key,cached);return Promise.resolve(cached)}
  return new Promise((resolve,reject)=>{
    const abort=()=>reject(abortError());signal.addEventListener('abort',abort,{once:true})
    queue.push({signal,run:async()=>{try{const result=await capture(path,stamp,time,signal);if(!signal.aborted){cache.set(key,result);if(cache.size>96)cache.delete(cache.keys().next().value!);resolve(result)}}catch(error){reject(error)}finally{signal.removeEventListener('abort',abort)}}});drain()
  })
}
function capture(path:string,stamp:string,time:number,signal:AbortSignal):Promise<VideoPoster>{
  return new Promise((resolve,reject)=>{
    if(signal.aborted){reject(abortError());return}
    const video=document.createElement('video');let done=false,target=Math.max(0,Number.isFinite(time)?time:0.1)
    const finish=(result?:VideoPoster,error?:Error)=>{if(done)return;done=true;clearTimeout(timer);signal.removeEventListener('abort',abort);video.onloadedmetadata=null;video.onloadeddata=null;video.onseeked=null;video.onerror=null;video.pause();video.removeAttribute('src');video.load();if(result)resolve(result);else reject(error||new Error('截图暂不可用'))}
    const abort=()=>finish(undefined,abortError()),timer=setTimeout(()=>finish(undefined,new Error('截图读取超时')),12000)
    const draw=()=>{if(done||video.seeking||video.readyState<2||!video.videoWidth||Math.abs(video.currentTime-target)>.15)return;try{const scale=Math.min(360/video.videoWidth,240/video.videoHeight,1),canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(video.videoWidth*scale));canvas.height=Math.max(1,Math.round(video.videoHeight*scale));const context=canvas.getContext('2d');if(!context)throw Error('无法读取截图');context.drawImage(video,0,0,canvas.width,canvas.height);finish({src:canvas.toDataURL('image/jpeg',.76),time:video.currentTime})}catch(error){finish(undefined,error as Error)}}
    video.muted=true;video.volume=0;video.preload='metadata';video.playsInline=true;video.disablePictureInPicture=true;video.disableRemotePlayback=true
    video.onloadedmetadata=()=>{target=Number.isFinite(video.duration)?Math.min(target,Math.max(0,video.duration-.05)):target;try{if(Math.abs(video.currentTime-target)>.01)video.currentTime=target;else draw()}catch(error){finish(undefined,error as Error)}}
    video.onloadeddata=draw;video.onseeked=draw;video.onerror=()=>finish(undefined,new Error('浏览器无法读取截图'));signal.addEventListener('abort',abort,{once:true})
    video.src='/api/video?'+new URLSearchParams({p:path,v:stamp});video.load()
  })
}

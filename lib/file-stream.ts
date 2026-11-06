import fs from 'node:fs'

/** A cancelled browser must stop the file producer before it can enqueue another chunk. */
export function fileStream(file:string,options:{start:number;end:number;signal?:AbortSignal}):ReadableStream<Uint8Array> {
  const source=fs.createReadStream(file,{start:options.start,end:options.end,highWaterMark:64*1024})
  let stopped=false,detach=()=>{}
  return new ReadableStream<Uint8Array>({
    start(controller){
      source.pause()
      const finish=()=>{if(stopped)return;stopped=true;detach();controller.close()}
      const abort=()=>{finish();source.destroy()}
      detach=()=>options.signal?.removeEventListener('abort',abort)
      source.on('data',(chunk:Buffer|string)=>{
        if(stopped)return
        controller.enqueue(typeof chunk==='string'?Buffer.from(chunk):chunk)
        if((controller.desiredSize??0)<=0)source.pause()
      })
      source.on('error',error=>{if(stopped)return;stopped=true;detach();controller.error(error)})
      source.once('end',finish)
      source.once('close',()=>{finish();detach()})
      if(options.signal?.aborted)abort()
      else options.signal?.addEventListener('abort',abort,{once:true})
    },
    pull(){if(!stopped)source.resume()},
    cancel(){stopped=true;detach();source.destroy()},
  },{highWaterMark:64*1024,size:chunk=>chunk.byteLength})
}

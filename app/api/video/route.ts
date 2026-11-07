import fs from 'fs'
import path from 'path'
import { NextRequest } from 'next/server'
import { fileStream } from '@/lib/file-stream'
import { MEDIA2 } from '@/lib/env'
import { readVideoSources, videoSource } from '@/lib/video-sources'

export async function GET(req: NextRequest) {
  const input = req.nextUrl.searchParams.get('p') || ''
  const mapped = videoSource(input,readVideoSources())
  if(!mapped)return new Response('not found',{status:404})
  let real: string
  try { real = fs.realpathSync(/*turbopackIgnore: true*/ mapped) }
  catch { return new Response('not found', {status:404}) }
  let mediaRoot: string
  try { mediaRoot = fs.realpathSync(/*turbopackIgnore: true*/ MEDIA2) }
  catch { return new Response('not found', {status:404}) }
  if (!real.startsWith(mediaRoot + '/')) return new Response('forbidden', {status:403})
  const st = fs.statSync(/*turbopackIgnore: true*/ real), size = st.size
  if (!st.isFile() || !size) return new Response('not found', {status:404})
  const types:Record<string,string> = {'.mp4':'video/mp4','.m4v':'video/mp4','.webm':'video/webm','.mov':'video/quicktime','.mkv':'video/x-matroska'}
  const headers:Record<string,string> = {'Content-Type':types[path.extname(real).toLowerCase()]||'application/octet-stream','Accept-Ranges':'bytes'}
  const requested = req.headers.get('range')
  let start=0,end=size-1,status=200
  if (requested) {
    const m = /^bytes=(\d*)-(\d*)$/.exec(requested)
    if (!m || (!m[1]&&!m[2])) return new Response('invalid range',{status:416,headers:{'Content-Range':`bytes */${size}`}})
    if (!m[1]) start=Math.max(0,size-Number(m[2]))
    else {start=Number(m[1]);if(m[2])end=Math.min(Number(m[2]),size-1)}
    if (!Number.isSafeInteger(start)||!Number.isSafeInteger(end)||start<0||start>=size||end<start) return new Response('invalid range',{status:416,headers:{'Content-Range':`bytes */${size}`}})
    headers['Content-Range']=`bytes ${start}-${end}/${size}`;status=206
  }
  headers['Content-Length']=String(end-start+1)
  return new Response(fileStream(real,{start,end,signal:req.signal}),{status,headers})
}

import fs from 'fs'
import path from 'path'
import Database from 'better-sqlite3'
import { NextRequest, NextResponse } from 'next/server'
import { BASE, MEDIA2 } from '@/lib/env'
import { loadReport } from '@/lib/integration'
import { loadArchive, videoInfo } from '@/lib/review-store'
import { readVideoSources, videoSource, isMediaFile } from '@/lib/video-sources'
import { creatorFolder } from '@/lib/source-resolution'
import { thumbUrl } from '@/lib/db'
import { healthScope } from '@/lib/video-health'

export async function GET(req:NextRequest) {
  const name=req.nextUrl.searchParams.get('name')?.trim()
  if(!name)return NextResponse.json({error:'name required'},{status:400})
  const identity=(loadReport().identities||[]).find((i:any)=>i.name===name)
  const names=new Set([name,...(identity?.aliases||[])].map((s:string)=>s.toLowerCase()))
  const sources=readVideoSources()
  const hidden=healthScope()
  for(const item of loadArchive())sources.set(item.vid,item.win_dst)
  const byPath=new Map<string,{vid:string;path:string}>()
  for(const [vid,stored]of sources) {
    const candidateFolders=[stored&&creatorFolder(stored,MEDIA2),creatorFolder(vid,MEDIA2)]
    if(!candidateFolders.some(folder=>folder&&names.has(folder.toLowerCase())))continue
    const source=videoSource(vid,sources),folder=source&&creatorFolder(source,MEDIA2)
    if(source&&folder&&names.has(folder.toLowerCase())&&isMediaFile(source)&&!hidden(vid,source)&&!byPath.has(source))byPath.set(source,{vid,path:source})
  }
  const candidates=[...byPath.values()].sort((a,b)=>a.path.localeCompare(b.path,'zh-CN'))
  const metadata=videoInfo(),db=new Database(`${BASE}/faces.db`,{readonly:true})
  try {
    const thumb=db.prepare('SELECT thumb FROM faces WHERE vid=? ORDER BY score DESC LIMIT 1')
    return NextResponse.json({name,total:candidates.length,vids:candidates.slice(0,12).map(v=>{
      const info=metadata.get(v.vid),image=info?.thumb||(thumb.get(v.vid)as any)?.thumb
      return {...v,name:path.basename(v.path),size_gb:+(fs.statSync(v.path).size/2**30).toFixed(2),duration:info?.duration,thumb:thumbUrl(image),state:'archived',organized:true}
    })})
  }finally{db.close()}
}

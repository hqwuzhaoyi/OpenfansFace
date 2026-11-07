import fs from 'node:fs/promises'
import type { Dirent } from 'node:fs'
import path from 'node:path'
import { CREATORS } from './env'
import type { PersonDirectoryStats } from './person-library-model'

export const personVideoExtension=/\.(mp4|m4v|mov|mkv|webm|avi|wmv|ts|mts|m2ts)$/i
type Result={stats:Record<string,PersonDirectoryStats>;updated_at:string}
let cached:{value:Result;expires:number;rootStamp:string}|undefined,pending:Promise<Result>|undefined,pendingStamp='',generation=0
export function invalidatePersonStats(){generation++;cached=undefined;pending=undefined}
export async function personDirectoryStats(force=false):Promise<Result>{
  let rootStamp='missing'
  try{const root=await fs.stat(CREATORS,{bigint:true});rootStamp=`${root.dev}:${root.ino}:${root.mtimeNs}`}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error}
  if(!force&&cached&&cached.expires>Date.now()&&cached.rootStamp===rootStamp)return cached.value
  if(pending){if(pendingStamp===rootStamp)return pending;await pending.catch(()=>{});return personDirectoryStats(force)}
  const token=generation
  const work=(async()=>{
    const stats:Record<string,PersonDirectoryStats>=Object.create(null)
    let entries:Dirent[]
    try{entries=await fs.readdir(CREATORS,{withFileTypes:true})}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;entries=[]}
    const folders=entries.filter(entry=>entry.isDirectory()&&!entry.isSymbolicLink()&&!entry.name.startsWith('.'))
    async function count(root:string):Promise<{video_count:number;total_bytes:number}>{
      let video_count=0,total_bytes=0
      for(const entry of await fs.readdir(root,{withFileTypes:true})){
        if(entry.isSymbolicLink()||entry.name.startsWith('.'))continue
        const file=path.join(root,entry.name)
        if(entry.isDirectory()){const nested=await count(file);video_count+=nested.video_count;total_bytes+=nested.total_bytes}
        else if(entry.isFile()&&personVideoExtension.test(entry.name)){
          try{const stat=await fs.stat(file);if(stat.isFile()){video_count++;total_bytes+=stat.size}}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error}
        }
      }
      return {video_count,total_bytes}
    }
    let cursor=0
    await Promise.all(Array.from({length:Math.min(4,folders.length)},async()=>{
      while(cursor<folders.length){const entry=folders[cursor++];try{stats[entry.name]=await count(path.join(CREATORS,entry.name))}catch{stats[entry.name]={video_count:null,total_bytes:null,stats_error:'目录暂时无法读取'}}}
    }))
    const value={stats,updated_at:new Date().toISOString()}
    if(token===generation)cached={value,expires:Date.now()+60_000,rootStamp}
    return value
  })()
  pending=work;pendingStamp=rootStamp
  try{return await work}finally{if(pending===work)pending=undefined}
}

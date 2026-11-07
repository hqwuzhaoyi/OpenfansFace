import fs from 'node:fs'
import path from 'node:path'
import { alternateName } from './archive-files'
import { ReviewError } from './review-logic'
const normalize=(name:string)=>name.normalize('NFC').toLowerCase()

/** A directory merge keeps both files on collision, including file-versus-directory collisions. */
export function mergedDirectoryPaths(source:string,target:string){
  const files:{src:string;dst:string;renamed:boolean}[]=[],directories:{src:string;dst:string}[]=[]
  const reserved=new Set<string>(),existing=new Map<string,Map<string,{name:string;directory:boolean}>>()
  function entry(file:string){
    const parent=path.dirname(file)
    if(!existing.has(parent)){
      const children=new Map<string,{name:string;directory:boolean}>()
      if(fs.existsSync(/*turbopackIgnore: true*/ parent))for(const child of fs.readdirSync(/*turbopackIgnore: true*/ parent,{withFileTypes:true}))children.set(normalize(child.name),{name:child.name,directory:child.isDirectory()&&!child.isSymbolicLink()})
      existing.set(parent,children)
    }
    return existing.get(parent)!.get(normalize(path.basename(file)))
  }
  function unique(desired:string,src:string){
    let result=desired,index=0
    while(entry(result)||reserved.has(normalize(result)))result=path.join(path.dirname(desired),alternateName(path.basename(desired),`${src}:${index++}`))
    reserved.add(normalize(result));return result
  }
  function walk(from:string,to:string){
    directories.push({src:from,dst:to})
    for(const item of fs.readdirSync(/*turbopackIgnore: true*/ from,{withFileTypes:true}).sort((a,b)=>a.name.localeCompare(b.name,'zh-CN'))){
      const src=path.join(from,item.name),desired=path.join(to,item.name)
      if(item.isSymbolicLink())throw new ReviewError('人物目录含符号链接，请先核对后再合并',409)
      if(item.isDirectory()){
        const same=entry(desired)
        const destination=same?.directory?path.join(to,same.name):unique(desired,src)
        walk(src,destination)
      }else if(item.isFile()){
        const dst=unique(desired,src)
        files.push({src,dst,renamed:dst!==path.join(target,path.relative(source,src))})
      }else throw new ReviewError('人物目录含特殊文件，不能合并',409)
    }
  }
  if(fs.existsSync(/*turbopackIgnore: true*/ source))walk(source,target)
  return {files,directories}
}

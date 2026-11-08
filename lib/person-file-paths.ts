import fs from 'node:fs'
import path from 'node:path'
import { createHash } from 'node:crypto'
import { CREATORS, STAGE } from './env'
import { ReviewError, validateCreator } from './review-logic'
import { validateArchiveName } from './archive-paths'

export function fingerprint(file:string):string {
  const stat=fs.lstatSync(/*turbopackIgnore: true*/ file,{bigint:true})
  if(!stat.isFile()||stat.isSymbolicLink())throw new ReviewError('只允许操作普通文件',409)
  // Creating/removing a hard link changes ctime, not the content or identity.
  return createHash('sha256').update(JSON.stringify([path.resolve(file),String(stat.dev),String(stat.ino),String(stat.size),String(stat.mtimeNs)])).digest('hex')
}
export function personDirectory(name:string):string {
  const valid=validateCreator(name)
  validateArchiveName(valid)
  if(valid.startsWith('.'))throw new ReviewError('人物名不能以点开头')
  return path.join(CREATORS,valid)
}
/** Reject symlinks at every existing component, not just at the leaf. */
export function guardedPath(value:string,root=CREATORS,allowMissing=false):string {
  const absolute=path.resolve(value),base=path.resolve(root),relative=path.relative(base,absolute)
  if(relative.startsWith('..')||path.isAbsolute(relative))throw new ReviewError('文件路径超出人物目录')
  let current=base
  const baseStat=fs.lstatSync(/*turbopackIgnore: true*/ base)
  if(baseStat.isSymbolicLink()||!baseStat.isDirectory())throw new ReviewError('媒体目录不是普通目录',409)
  for(const segment of relative.split(path.sep).filter(Boolean)) {
    current=path.join(current,segment)
    try{if(fs.lstatSync(/*turbopackIgnore: true*/ current).isSymbolicLink())throw new ReviewError('不允许通过符号链接操作文件',409)}
    catch(error){if(allowMissing&&(error as NodeJS.ErrnoException).code==='ENOENT')continue;throw error}
  }
  return absolute
}
export function safeSubfolder(input:unknown):string {
  if(input===undefined||input==='')return ''
  if(typeof input!=='string'||input.startsWith('/')||input.includes('\\'))throw new ReviewError('请输入相对人物目录的子文件夹')
  try{for(const segment of input.split('/'))if(!segment||segment.startsWith('.')||validateArchiveName(segment)!==segment)throw new Error('子文件夹路径无效')}catch(error){throw new ReviewError((error as Error).message)}
  return input
}
export function isStagedPath(file:string):boolean {const relative=path.relative(path.resolve(STAGE),path.resolve(file));return relative===''||(!relative.startsWith('..')&&!path.isAbsolute(relative))}

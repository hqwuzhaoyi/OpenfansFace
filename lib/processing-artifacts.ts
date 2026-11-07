import fs from 'node:fs'
import path from 'node:path'
import Database from 'better-sqlite3'
import {BASE} from './env'
import {readVideoSources,videoSource} from './video-sources'

/** Only the native transaction receipt grants access to immutable attempt bytes. */
export function acceptedAttemptFile(attempt:string,relative:string):string|null {
  if(!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(attempt))return null
  let directory:string
  try{directory=path.join(fs.realpathSync(BASE),'processing/attempts',attempt)}catch{return null}
  let db:Database.Database|undefined
  try {
    db=new Database(path.join(BASE,'processing/processing.db'),{readonly:true,fileMustExist:true})
    const row=db.prepare('SELECT result FROM _face_admin_processing_results WHERE id=?').get(attempt) as {result:string}|undefined
    if(!row)return null
    const receipt=JSON.parse(row.result),task=receipt.task
    if(receipt.ok!==true||receipt.accepted!==true||task?.run_id!==attempt)return null
    const current=fs.realpathSync(videoSource(receipt.vid,readVideoSources())||task.path)
    const source=fs.statSync(current,{bigint:true}),identity=receipt.source_identity
    if(!identity||source.dev.toString()!==String(identity.dev)||source.ino.toString()!==String(identity.ino)||Number(source.size)!==task.size||source.mtimeNs.toString()!==task.mtime_ns)return null
    const file=path.resolve(directory,relative)
    const face=receipt.data?.face,watermark=receipt.data?.watermark
    const referenced=face?.thumbs?.includes(file)||(watermark?.artifact_attempt===attempt&&watermark?.candidates?.some((candidate:{evidence:string})=>relative===path.join('watermarks/evidence',candidate.evidence)))
    if(!referenced||!file.startsWith(directory+path.sep)||fs.realpathSync(file)!==file||!fs.statSync(file).isFile())return null
    return file
  }catch{return null}finally{db?.close()}
}
export function acceptedAttemptThumbnail(file:string):boolean {
  try {
    const root=path.join(fs.realpathSync(BASE),'processing/attempts')+path.sep
    const canonical=fs.realpathSync(file)
    if(!canonical.startsWith(root))return true
    const [attempt,folder,name,...extra]=canonical.slice(root.length).split(path.sep)
    if(folder!=='thumbs'||extra.length||!name||!name.endsWith('.jpg'))return false
    return acceptedAttemptFile(attempt,path.join(folder,name))===canonical
  }catch{return false}
}

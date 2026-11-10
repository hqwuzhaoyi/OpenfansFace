import fs from 'node:fs'
import path from 'node:path'
import {BASE} from './env'
import {nativeJsonRequest} from './native-json-transport'

/** Only the native transaction receipt grants access to immutable attempt bytes. */
export async function acceptedAttemptFile(attempt:string,relative:string):Promise<string|null> {
  if(!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(attempt))return null
  let directory:string
  try{directory=path.join(fs.realpathSync(BASE),'processing/attempts',attempt)}catch{return null}
  try {
    const res=await nativeJsonRequest<{accepted?:boolean;file?:string|null}>(
      'v1/metadata',
      {command:'processing-artifact-check',body:{attempt,relative}},
      {
        label:'尝试产物回执校验',
        completion:'required',
        error:(message,status)=>Object.assign(new Error(message),{status}),
      },
    )
    if(res?.accepted!==true||typeof res.file!=='string')return null
    const file=path.resolve(directory,relative)
    if(res.file!==file||!file.startsWith(directory+path.sep)||fs.realpathSync(file)!==file||!fs.statSync(file).isFile())return null
    return file
  }catch{return null}
}
export async function acceptedAttemptThumbnail(file:string):Promise<boolean> {
  try {
    const root=path.join(fs.realpathSync(BASE),'processing/attempts')+path.sep
    const canonical=fs.realpathSync(file)
    if(!canonical.startsWith(root))return true
    const [attempt,folder,name,...extra]=canonical.slice(root.length).split(path.sep)
    if(folder!=='thumbs'||extra.length||!name||!name.endsWith('.jpg'))return false
    return (await acceptedAttemptFile(attempt,path.join(folder,name)))===canonical
  }catch{return false}
}

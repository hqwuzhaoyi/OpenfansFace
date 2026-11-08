import { rustRequest } from './file-operation-backend'
import type { PersonLibrary, PersonVideo } from './person-library-model'

interface NativeVideo extends Omit<PersonVideo,'modified_at'> { modified_ms:number }
interface NativeInventory extends Pick<PersonLibrary,'name'|'root'|'folders'|'directory_exists'> {files:NativeVideo[]}
export async function nativeLibraryNames():Promise<string[]> {
  return (await rustRequest<{names:string[]}>(null,undefined,'v1/person-library')).names
}
export async function nativePersonInventory(name:string):Promise<Pick<PersonLibrary,'name'|'root'|'files'|'folders'|'directory_exists'>> {
  const result=await rustRequest<NativeInventory>(null,undefined,'v1/person-library?'+new URLSearchParams({name}))
  return {...result,files:result.files.map(({modified_ms,...file})=>({...file,modified_at:new Date(modified_ms).toISOString()}))}
}
export interface NativeObservation {status:string;eligible:boolean;assigned?:boolean;path:string|null;size?:number;mtime_ns?:string;modified_ms?:number;version?:string}
export async function nativeLibraryObservations(ids:Iterable<string>):Promise<Record<string,NativeObservation>> {
  const selected=[...new Set(ids)],observations:Record<string,NativeObservation>=Object.create(null)
  // Sequential bounded batches avoid letting one page occupy every media reader.
  for(let offset=0;offset<selected.length;offset+=512){
    const result=await rustRequest<{observations:Record<string,NativeObservation>}>({vids:selected.slice(offset,offset+512)},undefined,'v1/person-library/observations')
    Object.assign(observations,result.observations)
  }
  return observations
}

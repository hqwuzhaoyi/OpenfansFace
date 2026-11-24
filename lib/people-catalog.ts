import path from 'path'
import { BASE } from './env'
import { loadReport } from './integration'
import { loadQueue,readJson,readCanonicalJson,writeJson } from './jsonio'
import { loadActiveArchive } from './review-store'
import { validateCreator } from './review-logic'
import { savePerson,mergePeople,type SavedPerson } from './people-model'
import { canonicalPersonName, personRenames } from './person-names'
import { nativeLibraryNames } from './person-library-backend'

export const PEOPLE_F=path.join(BASE,'people-manual.json')
export const loadSavedPeople=()=>readCanonicalJson<SavedPerson[]>(PEOPLE_F,[])
/** Caller holds the review write lock. Person existence is independent of a naming batch. */
export function rememberPerson(name:string,thumb?:string|null) {
  const valid=canonicalPersonName(validateCreator(name))
  const deleted={...readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})}
  if(Object.hasOwn(deleted,valid)){delete deleted[valid];writeJson(path.join(BASE,'people-deleted.json'),deleted)}
  writeJson(PEOPLE_F,savePerson(loadSavedPeople(),valid,new Date().toISOString(),thumb))
}
export function publicThumb(value?:string|null):string|null {
  if(!value)return null
  return value.startsWith('/thumb?')?value:`/thumb?p=${encodeURIComponent(value)}`
}
function reverseRenameAliases(renamed:Record<string,string>):Map<string,string[]> {
  const map=new Map<string,string[]>()
  for(const alias of Object.keys(renamed)){
    const current=canonicalPersonName(alias,renamed)
    if(!alias||alias===current)continue
    map.set(current,[...(map.get(current)||[]),alias])
  }
  return map
}
export function peopleCatalog() {
  const report=loadReport()
  const assigned=[...loadQueue(),...loadActiveArchive()].map(v=>({name:v.creator,rep_thumb:v.thumb||null,recent_at:v.at}))
  const deleted=readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})
  const reverseAliases=reverseRenameAliases(personRenames())
  return mergePeople(report.identities||[],loadSavedPeople(),assigned)
    .filter(p=>!Object.hasOwn(deleted,p.name))
    .map(p=>{
      const aliases=[...new Set([...(p.aliases||[]),...(reverseAliases.get(p.name)||[])].filter(a=>a&&a!==p.name))]
      return {...p,rep_thumb:publicThumb(p.rep_thumb),...(aliases.length?{aliases}:{})}
    })
}
export async function pickerPeopleCatalog() {
  const base=peopleCatalog()
  const people=new Map(base.map(p=>[p.name,p]))
  const deleted=readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})
  const reverseAliases=reverseRenameAliases(personRenames())
  let dirNames:string[]=[]
  try{dirNames=await nativeLibraryNames()}catch{dirNames=[]}
  const added:typeof base=[]
  for(const rawName of dirNames){
    const name=canonicalPersonName(rawName)
    if(!name||Object.hasOwn(deleted,name))continue
    if(people.has(name)){
      if(rawName!==name){
        const existing=people.get(name)!
        const aliases=[...new Set([...(existing.aliases||[]),rawName].filter(a=>a&&a!==name))]
        if(aliases.length)people.set(name,{...existing,aliases})
      }
      continue
    }
    const aliases=[...new Set([...(reverseAliases.get(name)||[]),...(rawName!==name?[rawName]:[])].filter(a=>a&&a!==name))]
    const entry={name,rep_thumb:null,...(aliases.length?{aliases}:{})}
    people.set(name,entry)
    added.push(entry)
  }
  added.sort((a,b)=>a.name.localeCompare(b.name,'zh-CN'))
  return [...base.map(p=>people.get(p.name)!),...added]
}

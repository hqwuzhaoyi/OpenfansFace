import path from 'path'
import { BASE } from './env'
import { loadReport } from './integration'
import { loadQueue,readJson,writeJson } from './jsonio'
import { loadArchive } from './review-store'
import { validateCreator } from './review-logic'
import { savePerson,mergePeople,type SavedPerson } from './people-model'
import { canonicalPersonName, canonicalPersonFields } from './person-names'

export const PEOPLE_F=path.join(BASE,'people-manual.json')
export const loadSavedPeople=()=>canonicalPersonFields(readJson<SavedPerson[]>(PEOPLE_F,[]))
/** Caller holds the review write lock. Person existence is independent of a naming batch. */
export function rememberPerson(name:string,thumb?:string|null) {
  const valid=canonicalPersonName(validateCreator(name))
  const deleted=readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})
  if(Object.hasOwn(deleted,valid)){delete deleted[valid];writeJson(path.join(BASE,'people-deleted.json'),deleted)}
  writeJson(PEOPLE_F,savePerson(loadSavedPeople(),valid,new Date().toISOString(),thumb))
}
export function publicThumb(value?:string|null):string|null {
  if(!value)return null
  return value.startsWith('/thumb?')?value:`/thumb?p=${encodeURIComponent(value)}`
}
export function peopleCatalog() {
  const report=loadReport()
  const assigned=[...loadQueue(),...loadArchive()].map(v=>({name:v.creator,rep_thumb:v.thumb||null,recent_at:v.at}))
  const deleted=readJson<Record<string,unknown>>(path.join(BASE,'people-deleted.json'),{})
  return mergePeople(report.identities||[],loadSavedPeople(),assigned).filter(p=>!Object.hasOwn(deleted,p.name)).map(p=>({...p,rep_thumb:publicThumb(p.rep_thumb)}))
}

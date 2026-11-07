import fs from 'node:fs'
import path from 'node:path'
import { BASE } from './env'

export const PERSON_NAMES_FILE=path.join(BASE,'person-renames.json')
export function personRenames():Record<string,string>{try{return JSON.parse(fs.readFileSync(PERSON_NAMES_FILE,'utf8'))}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return {};throw error}}
export function canonicalPersonName(name:string,names=personRenames()):string {
  const seen=new Set<string>()
  while(Object.hasOwn(names,name)&&typeof names[name]==='string'&&!seen.has(name)){seen.add(name);name=names[name]}
  return name
}
/** Rewrite semantic person fields only; IDs and original evidence stay immutable. */
export function canonicalPersonFields<T>(value:T,names=personRenames()):T {
  function visit(v:any,key=''):any {
    if(typeof v==='string'&&['name','creator','suggested'].includes(key))return canonicalPersonName(v,names)
    if(Array.isArray(v))return v.map(x=>visit(x))
    if(v&&typeof v==='object')return Object.fromEntries(Object.entries(v).map(([k,x])=>[k,visit(x,k)]))
    return v
  }
  return visit(value)
}

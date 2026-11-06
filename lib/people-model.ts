export interface SavedPerson { name:string;created_at:string;last_used_at:string;rep_thumb?:string|null }
export interface PersonSeed { name:string;rep_thumb?:string|null;recent_at?:string }
export function savePerson(rows:SavedPerson[],name:string,at:string,thumb?:string|null):SavedPerson[] {
  const old=rows.find(p=>p.name===name)
  const updated={name,created_at:old?.created_at||at,last_used_at:at,rep_thumb:old?.rep_thumb||thumb||null}
  return [...rows.filter(p=>p.name!==name),updated]
}
export function mergePeople(report:PersonSeed[],saved:SavedPerson[],assigned:PersonSeed[]):PersonSeed[] {
  const people=new Map<string,PersonSeed>()
  const timestamp=(s?:string)=>s?Date.parse(s.includes('T')?s:s.replace(' ','T')+'Z')||0:0
  const merge=(p:PersonSeed)=>{
    const old=people.get(p.name)
    people.set(p.name,{name:p.name,rep_thumb:old?.rep_thumb||p.rep_thumb||null,recent_at:timestamp(p.recent_at)>timestamp(old?.recent_at)?p.recent_at:old?.recent_at})
  }
  report.forEach(merge);saved.forEach(p=>merge({...p,recent_at:p.last_used_at}));assigned.forEach(merge)
  return [...people.values()].sort((a,b)=>timestamp(b.recent_at)-timestamp(a.recent_at)||a.name.localeCompare(b.name,'zh-CN'))
}

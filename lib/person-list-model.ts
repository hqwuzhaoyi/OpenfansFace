import type { LibraryPerson } from './person-library-model'
export type PersonSort='recent'|'name-asc'|'name-desc'|'videos-desc'|'videos-asc'|'bytes-desc'|'bytes-asc'
export function matchesPersonQuery(person:{name:string;aliases?:string[]},query:string){
  const words=query.trim().normalize('NFKC').toLowerCase().split(/\s+/).filter(Boolean)
  const text=[person.name,...(person.aliases||[])].join(' ').normalize('NFKC').toLowerCase()
  return words.every(word=>text.includes(word))
}
export function sortedLibraryPeople(people:LibraryPerson[],query:string,sort:PersonSort):LibraryPerson[]{
  const filtered=people.filter(person=>matchesPersonQuery(person,query))
  return filtered.sort((a,b)=>{
    let order=0
    if(sort.startsWith('videos')||sort.startsWith('bytes')){
      const field=sort.startsWith('videos')?'video_count':'total_bytes',x=a[field],y=b[field]
      if(x==null||y==null){if(x==null&&y!=null)return 1;if(y==null&&x!=null)return -1}
      else order=(sort.endsWith('asc')?1:-1)*(x-y)
    }else if(sort==='recent')order=(Date.parse(b.recent_at||'')||0)-(Date.parse(a.recent_at||'')||0)
    else if(sort==='name-desc')return b.name.localeCompare(a.name,'zh-CN',{numeric:true})
    return order||a.name.localeCompare(b.name,'zh-CN',{numeric:true})
  })
}

import type { CreatorMatch, CreatorProfile } from './creator-directory-model'

export interface PersonAccountLink {
  id:string; creator:string; profile:CreatorProfile; linked_at:string; source_fetched_at:string
}
export interface PersonAccountStore {version:1;source:'nv-pu-sa';links:PersonAccountLink[]}
export function parsePersonAccounts(value:unknown):PersonAccountLink[] {
  if(!value||typeof value!=='object'||!('version'in value)||value.version!==1||!('source'in value)||value.source!=='nv-pu-sa'||!('links'in value)||!Array.isArray(value.links))throw new Error('手动账号关联记录格式不正确')
  const result:PersonAccountLink[]=[]
  const ids=new Set<string>(),pairs=new Set<string>()
  for(const row of value.links){
    if(!row||typeof row.id!=='string'||typeof row.creator!=='string'||!row.creator.trim()||!row.profile||typeof row.profile.id!=='string'||!/^\d+$/.test(row.profile.id)||typeof row.profile.handle!=='string'||!/^[A-Za-z0-9_]{1,15}$/.test(row.profile.handle)||typeof row.profile.name!=='string'||typeof row.profile.description!=='string'||!Number.isFinite(Date.parse(row.linked_at))||!Number.isFinite(Date.parse(row.source_fetched_at)))throw new Error('手动账号关联记录损坏，请核对后重试')
    const pair=JSON.stringify([row.creator,row.profile.id])
    if(ids.has(row.id)||pairs.has(pair))throw new Error('手动账号关联记录存在重复，请核对后重试')
    ids.add(row.id);pairs.add(pair);result.push(row)
  }
  return result
}
/** User-selected associations outrank discovery hints, without becoming watermark rules. */
export function manualAccountMatches(links:PersonAccountLink[],creator:string,profiles:CreatorProfile[]=[]):CreatorMatch[] {
  const current=new Map(profiles.map(profile=>[profile.id,profile]))
  return links.filter(link=>link.creator===creator).map(link=>({profile:current.get(link.profile.id)||link.profile,basis:'manual-link',manual_link_id:link.id,reviewed_at:link.linked_at,evidence:`手动确认关联到「${creator}」；视频归属仍需人工核对`}))
}

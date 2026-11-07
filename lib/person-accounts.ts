import { nativeMetadataEnabled, nativeMetadata } from './shared-metadata-backend'
import fs from 'node:fs'
import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { canonicalPersonName } from './person-names'
import { libraryPeople } from './person-library'
import { acquireReviewLock } from './review-store'
import { ReviewError, validateCreator } from './review-logic'
import { writeJson } from './jsonio'
import { creatorDirectory } from './creator-directory'
import { PERSON_ACCOUNTS_FILE, loadPersonAccounts } from './person-account-store'
import { type PersonAccountLink } from './person-account-model'

export { PERSON_ACCOUNTS_FILE, loadPersonAccounts } from './person-account-store'
function save(links:PersonAccountLink[]){fs.mkdirSync(path.dirname(PERSON_ACCOUNTS_FILE),{recursive:true});writeJson(PERSON_ACCOUNTS_FILE,{version:1,source:'nv-pu-sa',links})}
export async function associatePersonAccount(body:{name?:unknown;profile_id?:unknown;handle?:unknown}) {
  if(!body||typeof body!=='object')throw new ReviewError('缺少账号关联信息')
  const name=canonicalPersonName(validateCreator(body.name))
  if(typeof body.profile_id!=='string'||!/^\d+$/.test(body.profile_id)||typeof body.handle!=='string'||!/^[A-Za-z0-9_]{1,15}$/.test(body.handle))throw new ReviewError('请选择要关联的账号')
  const directory=await creatorDirectory(),profile=directory.profiles.find(p=>p.id===body.profile_id)
  if(!profile||profile.handle.toLowerCase()!==body.handle.toLowerCase())throw new ReviewError('账号资料已变化，请重新搜索并确认',409)
  if(nativeMetadataEnabled())return (await nativeMetadata<{link:PersonAccountLink}>('account-link',{name,profile_id:body.profile_id,handle:body.handle})).link
  const release=acquireReviewLock()
  try{
    const creator=canonicalPersonName(name)
    if(!(await libraryPeople()).some(p=>p.name===creator))throw new ReviewError('人物不存在，请刷新人物列表',404)
    const links=loadPersonAccounts(),existing=links.find(link=>link.creator===creator&&link.profile.id===profile.id)
    if(existing)return existing
    const link:PersonAccountLink={id:randomUUID(),creator,profile,linked_at:new Date().toISOString(),source_fetched_at:directory.fetched_at}
    save([...links,link]);return link
  }finally{release()}
}
export async function unlinkPersonAccount(body:{name?:unknown;id?:unknown}) {
  if(!body||typeof body!=='object')throw new ReviewError('缺少关联记录')
  const creator=canonicalPersonName(validateCreator(body.name))
  if(typeof body.id!=='string'||!body.id)throw new ReviewError('请选择要解除的关联')
  if(nativeMetadataEnabled()){await nativeMetadata('account-unlink',{name:creator,link_id:body.id});return}
  const release=acquireReviewLock()
  try{
    const links=loadPersonAccounts(),match=links.find(link=>link.id===body.id&&link.creator===creator)
    if(!match)throw new ReviewError('关联记录已变化，请刷新后重试',409)
    save(links.filter(link=>link.id!==match.id||link.creator!==creator))
  }finally{release()}
}

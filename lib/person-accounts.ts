import { nativeMetadataEnabled, nativeMetadata } from './shared-metadata-backend'
import { canonicalPersonName } from './person-names'
import { ReviewError, validateCreator } from './review-logic'
import { creatorDirectory } from './creator-directory'
import { type PersonAccountLink } from './person-account-model'

export { PERSON_ACCOUNTS_FILE, loadPersonAccounts } from './person-account-store'
export async function associatePersonAccount(body:{name?:unknown;profile_id?:unknown;handle?:unknown}) {
  if(!body||typeof body!=='object')throw new ReviewError('缺少账号关联信息')
  const name=canonicalPersonName(validateCreator(body.name))
  if(typeof body.profile_id!=='string'||!/^\d+$/.test(body.profile_id)||typeof body.handle!=='string'||!/^[A-Za-z0-9_]{1,15}$/.test(body.handle))throw new ReviewError('请选择要关联的账号')
  const directory=await creatorDirectory(),profile=directory.profiles.find(p=>p.id===body.profile_id)
  if(!profile||profile.handle.toLowerCase()!==body.handle.toLowerCase())throw new ReviewError('账号资料已变化，请重新搜索并确认',409)
  return (await nativeMetadata<{link:PersonAccountLink}>('account-link',{name,profile_id:body.profile_id,handle:body.handle})).link
}
export async function unlinkPersonAccount(body:{name?:unknown;id?:unknown}) {
  if(!body||typeof body!=='object')throw new ReviewError('缺少关联记录')
  const creator=canonicalPersonName(validateCreator(body.name))
  if(typeof body.id!=='string'||!body.id)throw new ReviewError('请选择要解除的关联')
  await nativeMetadata('account-unlink',{name:creator,link_id:body.id})
}

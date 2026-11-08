export interface PeachPersonMatch {id:string;name:string;kind:'creator'}
export type PeachPersonLookup = {status:'linked';person:PeachPersonMatch;base_url:string|null}|{status:'missing'|'ambiguous';base_url:string|null}

export function peachPublicBase(value?:string|null):string|null {
  if(!value?.trim())return null
  const url=new URL(value.trim())
  if(!['http:','https:'].includes(url.protocol)||url.username||url.password||url.search||url.hash)throw new Error('PEACH_PUBLIC_URL 必须是无凭据、无查询参数的 HTTP(S) 基础地址')
  if(!url.pathname.endsWith('/'))url.pathname+='/'
  return url.href
}
export function validatePeachPersonId(id:string):void {
  if(!/^[1-9][0-9]{0,18}$/.test(id)||BigInt(id)>=BigInt('9223372036854775808'))throw new Error('Peach 人物编号无效')
}
export function peachPersonHref(id:string,base:string|null,origin:string):string {
  validatePeachPersonId(id)
  const configured=peachPublicBase(base)
  const url=new URL(configured||origin)
  if(!configured){url.protocol='http:';url.port='8900';url.pathname='/';url.search='';url.hash=''}
  return new URL(`creators/${id}`,url).href
}

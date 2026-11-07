/** Shared query normalization: preserve text, clamp pages, reject unknown enums. */
export function readWorkspaceQuery<T extends Record<string,string>>(params:URLSearchParams,defaults:T,allowed:Partial<Record<keyof T,readonly string[]>>={}):T {
  return Object.fromEntries(Object.entries(defaults).map(([key,fallback])=>{
    let value=params.get(key)??fallback
    const choices=allowed[key as keyof T]
    if(choices&&!choices.includes(value))value=fallback
    if(key==='page')value=String(Math.max(0,Math.floor(Number.isFinite(Number(value))?Number(value):0)))
    return [key,value]
  })) as T
}
export function writeWorkspaceQuery(params:URLSearchParams,values:Record<string,string>,defaults:Record<string,string>){
  const next=new URLSearchParams(params)
  for(const [key,value]of Object.entries(values))if(value&&value!==defaults[key])next.set(key,value);else next.delete(key)
  return next
}

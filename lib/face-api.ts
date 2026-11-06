export class NetworkRequestError extends Error {
  path:string;method:string;resultUnknown:boolean
  constructor(path:string,method:string){
    super(method==='GET'?'连接中断，未能加载数据。请检查手机网络后重试。':'连接中断，操作结果尚未确认。请先查看待归档／归档记录，勿重复提交。')
    this.name='NetworkRequestError';this.path=path;this.method=method;this.resultUnknown=method!=='GET'
  }
}
export async function api<T=any>(path:string,opts?:RequestInit):Promise<T> {
  const method=(opts?.method||'GET').toUpperCase(),attempts=method==='GET'?2:1
  for(let attempt=0;attempt<attempts;attempt++){
    try{
      const headers=new Headers(opts?.headers);if(!headers.has('Content-Type'))headers.set('Content-Type','application/json')
      const res=await fetch(path,{...opts,headers})
      if(!res.ok){const message=await res.json().catch(()=>({}));throw new Error(message.error||`请求失败（${res.status}）`)}
      return await res.json() as T
    }catch(error){
      if(opts?.signal?.aborted||(error as Error)?.name==='AbortError')throw error
      if(!(error instanceof TypeError))throw error
      if(attempt+1<attempts){await new Promise(resolve=>setTimeout(resolve,200));continue}
      throw new NetworkRequestError(path,method)
    }
  }
  throw new NetworkRequestError(path,method)
}
export interface Stats { funnel: any; creators_dirs: number; pending_review: number; queue_len: number; staging: any; disk: any; rebuild: any }
export interface Person { name: string; claimed_n: number; claimed_gb: number; integrated_files: number; integrated_gb: number; tpl_faces: number; peach_links: number; aliases: string[]; rep_thumb: string }
export interface Cluster { cid: string; n_videos: number; total_gb: number; rep_thumb: string; collections: string[]; weak_count?: number }
export interface Claim { vid: string; path?:string; file_name?:string; name: string; score: number; n_faces: number; zone: string; thumb: string | null }

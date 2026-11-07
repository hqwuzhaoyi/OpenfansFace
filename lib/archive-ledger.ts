import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { peachExec } from './peach'
import { archiveLedgerProgram } from './archive-ledger-program'

export interface ArchiveLedgerRow { from:string; to:string; creator:string; canonical?:string }
export async function archiveLedger(action:'reserve'|'apply'|'status', id:string, rows:ArchiveLedgerRow[] = []) {
  const local=process.env.PEACH_LEDGER_PATH
  const encoded=Buffer.from(JSON.stringify({action,id,rows,database:local||'/data/database/ledger.db'})).toString('base64')
  if(encoded.length>(local&&process.platform==='darwin'?180_000:1_400_000))throw new Error('本次路径信息过多，请减少所选视频')
  const args=encoded.match(/.{1,24000}/g)||[]
  let timer:ReturnType<typeof setTimeout>|undefined
  try {
    const request=local
      ? promisify(execFile)(process.env.PYTHON_BIN||'python3',['-c',archiveLedgerProgram,...args],{timeout:45_000,maxBuffer:2**20}).then(r=>r.stdout).catch(e=>{throw new Error(e.stderr?.trim().split('\n').at(-1)||'账本连接失败')})
      : peachExec(archiveLedgerProgram,45_000,args).then(r=>{if(r.code!==0)throw new Error(r.err||'账本执行失败');return r.out})
    const output=await Promise.race([request,new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(new Error('Peach 账本请求超时')),45_000)})])
    return JSON.parse(output) as {state:'reserved'|'committed'|'cancelled';updated?:number}
  } finally {if(timer)clearTimeout(timer)}
}

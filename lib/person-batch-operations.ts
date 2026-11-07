import fs from 'node:fs'
import path from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import { BASE } from './env'
import { ReviewError, validateCreator } from './review-logic'
import { acquireReviewLock } from './review-store'
import { canonicalPersonName } from './person-names'
import { readJson, writeJson } from './jsonio'
import { previewPersonFiles, applyPersonFiles } from './person-file-operations'
import type { PersonBatchRequest, PersonBatchPlan, PersonBatchResult, PersonBatchProgress } from './person-batch-model'

interface Journal {plan:PersonBatchPlan;status:'preview'|'running'|'complete';result?:PersonBatchResult;active_person?:string}
function journalPath(id:string){
  if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(id))throw new ReviewError('批量操作编号无效')
  return path.join(BASE,'person-operations','batches',id+'.json')
}
export function personBatchProgress(id:string):PersonBatchProgress{
  const journal=readJson<Journal|null>(journalPath(id),null)
  if(!journal)throw new ReviewError('找不到此批次',404)
  return {status:journal.status,completed:journal.result?.completed.map(item=>item.person)||[],remaining:journal.result?.remaining||journal.plan.plans.map(plan=>plan.person),...(journal.status==='running'?{active_person:journal.active_person}:{}),error:journal.result?.error}
}
export async function previewPersonBatch(input:PersonBatchRequest):Promise<PersonBatchPlan>{
  if(!input||!['merge','delete'].includes(input.action)||!Array.isArray(input.people)||input.people.length>50)throw new ReviewError('请选择 1–50 个人物')
  const people=input.people.map(name=>canonicalPersonName(validateCreator(name)))
  if(new Set(people).size!==people.length||people.length<(input.action==='merge'?2:1))throw new ReviewError('合并至少选择两位不同人物，删除至少选择一位')
  const target=input.action==='merge'?canonicalPersonName(validateCreator(input.target)):undefined
  if(target&&!people.includes(target))throw new ReviewError('保留人物必须从已选人物中选择')
  const request:PersonBatchRequest={action:input.action,people,...(target?{target}:{})}
  const plans=[]
  for(const person of people.filter(person=>person!==target))plans.push(await previewPersonFiles({action:input.action==='merge'?'merge-person':'delete-person',person,...(target?{target_person:target}:{})}))
  const plan:PersonBatchPlan={id:randomUUID(),version:'',request,plans}
  plan.version=createHash('sha256').update(JSON.stringify(plan)).digest('hex')
  const file=journalPath(plan.id);fs.mkdirSync(path.dirname(file),{recursive:true});writeJson(file,{plan,status:'preview'} satisfies Journal)
  return plan
}
export async function applyPersonBatch(id:string,version:string):Promise<PersonBatchResult>{
  const file=journalPath(id),release=acquireReviewLock()
  try{
    const journal=readJson<Journal|null>(file,null)
    if(!journal||journal.plan.version!==version)throw new ReviewError('预览已失效，请重新预览',409)
    if(journal.status==='complete'&&journal.result)return journal.result
    if(journal.status!=='preview')throw new ReviewError('此批次仍在执行或需要核对，请刷新列表，不要重复提交',409)
    // Validate every original preview before changing the first person.
    for(const plan of journal.plan.plans){const current=await previewPersonFiles(plan.request);if(current.version!==plan.version)throw new ReviewError('人物、文件或账本已变化，请重新预览',409)}
    const result:PersonBatchResult={ok:false,completed:[],remaining:journal.plan.plans.map(plan=>plan.person),target:journal.plan.request.target}
    journal.status='running';journal.result=result;writeJson(file,journal)
    for(const original of journal.plan.plans){
      try{
        journal.active_person=original.person;writeJson(file,journal)
        // Earlier merges change shared metadata and can create name collisions.
        // Replan under the same write lock; keep both copies at the final paths.
        const current=await previewPersonFiles(original.request)
        const before=original.files.map(f=>[f.src,f.version]),after=current.files.map(f=>[f.src,f.version])
        if(JSON.stringify(before)!==JSON.stringify(after)||JSON.stringify(original.directories)!==JSON.stringify(current.directories))throw new ReviewError('来源文件在批量操作期间变化，已停止剩余操作',409)
        const saved=await applyPersonFiles(current.request,current.version,true)
        result.completed.push({person:original.person,result:saved});result.remaining.shift();writeJson(file,journal)
      }catch(error){result.error=(error as Error).message;break}
    }
    result.ok=!result.remaining.length;journal.status='complete';writeJson(file,journal)
    return result
  }finally{
    const lock=readJson<{status?:string}>(path.join(BASE,'.review-write.lock'),{})
    release(lock.status==='recovery_needed')
  }
}

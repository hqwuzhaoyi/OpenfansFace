import type { ProcessingTask, IdentityCandidate } from './processing-model'

type Reason={kind:'watermark'|'folder'|'face'|'other';text:string;confirmed:boolean}
const finite=(value:unknown):value is number=>typeof value==='number'&&Number.isFinite(value)
export function candidateReasons(candidate:IdentityCandidate):Reason[]{
  const reasons:Reason[]=[]
  for(const evidence of candidate.evidence||[]){
    if(evidence.source==='watermark')reasons.push({kind:'watermark',confirmed:evidence.confirmed===true,text:evidence.confirmed===true?`本轮命中已确认账号映射：${evidence.detail||'账号文字未记录'}`:evidence.detail?.startsWith('账号尚未确认')?evidence.detail:`账号尚未确认：${evidence.detail||'文字未记录'}`})
    else if(evidence.source==='folder')reasons.push({kind:'folder',confirmed:false,text:evidence.detail||'文件名或文件夹名称命中'})
    else if(evidence.source==='face')reasons.push({kind:'face',confirmed:false,text:`人脸相似度 ${finite(evidence.score)?evidence.score.toFixed(3):'未记录'}${finite(evidence.support)?` · ${evidence.support} 张脸支持`:''}`})
    else if(evidence.detail)reasons.push({kind:'other',confirmed:false,text:evidence.detail})
  }
  if(!reasons.length)reasons.push({kind:'other',confirmed:false,text:'候选依据未记录，请查看详情核对'})
  return [...new Map(reasons.map(reason=>[reason.kind+':'+reason.text,reason])).values()]
}
export function reviewListSummary(task:ProcessingTask){
  const candidates=(task.data.candidates||[]).map(candidate=>({name:candidate.name,reasons:candidateReasons(candidate)}))
  const conflict=task.data.kind==='conflict'||(!task.data.kind&&candidates.length>1)
  const evidence=(task.data.candidates||[]).flatMap(candidate=>candidate.evidence||[])
  const confirmed=new Set(evidence.filter(e=>e.source==='watermark'&&e.confirmed===true&&e.key).map(e=>e.key))
  const unconfirmed=new Set(evidence.filter(e=>e.source==='watermark'&&e.confirmed!==true&&e.key).map(e=>e.key))
  const marks=(task.data.watermark?.candidates||[]).map(mark=>({key:mark.key,text:mark.text,frames:finite(mark.frames)&&mark.frames>=0?mark.frames:null,time:finite(mark.timestamp)&&mark.timestamp>=0?mark.timestamp:null,label:confirmed.has(mark.key)?'已确认映射':unconfirmed.has(mark.key)?'账号待核对':mark.platform==='Telegram'?'频道线索':'文字线索',confirmed:confirmed.has(mark.key)}))
  marks.sort((a,b)=>(confirmed.has(a.key)?0:unconfirmed.has(a.key)?1:2)-(confirmed.has(b.key)?0:unconfirmed.has(b.key)?1:2))
  const sampleTime=marks.find(mark=>mark.time!==null)?.time??0.1
  const status=conflict?'候选冲突':task.manual?'人工接管':candidates.length?'待核对':'需选择人物'
  return {candidates,conflict,marks,sampleTime,status,warning:task.manual?(task.error||'部分分析未完成'):task.data.watermark?.status==='error'?'文字识别未完成':null}
}

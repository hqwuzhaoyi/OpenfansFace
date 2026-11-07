'use client'

// Business composition of the existing BEUI Card / Checkbox / Button and shared cover.
import { useState } from 'react'
import { ChevronRight, ScanText, AlertTriangle } from 'lucide-react'
import { Card } from '@/components/beui/motion/card'
import { Checkbox } from '@/components/beui/motion/checkbox'
import { WorkspaceButton, WorkspacePath } from '@/components/workbench/inner'
import { VideoCover } from './video-cover'
import { reviewListSummary } from '@/lib/review-list-summary'
import type { ProcessingTask } from '@/lib/processing-model'
import './review-summary.css'

export function ReviewSummaryRow({task,selected,active,disabled,onToggle,onInspect}:{task:ProcessingTask;selected:boolean;active:boolean;disabled:boolean;onToggle:()=>void;onInspect:()=>void}){
  const summary=reviewListSummary(task),candidates=summary.candidates.slice(0,2),marks=summary.marks.slice(0,2)
  const proof=marks[0],[failedProof,setFailedProof]=useState<string|null>(null)
  return <Card role="article" aria-label={task.name} size="compact" className="ws-row review-summary" data-vid={task.vid} data-active={active} selected={selected}>
    <Checkbox className="ws-check review-summary-check" aria-label={`选择 ${task.name}`} motionPreset="instant" checked={selected} disabled={disabled} onCheckedChange={onToggle}/>
    <div className="review-summary-media"><WorkspaceButton variant="ghost" className="review-summary-cover" aria-label={`查看 ${task.name} 的视频与详情`} title="查看视频与详情" onClick={onInspect}><VideoCover video={{path:task.path,thumb:null,size:task.size,mtime_ns:task.mtime_ns}} sampleTime={summary.sampleTime}/></WorkspaceButton>{proof&&<WorkspaceButton variant="ghost" className="review-mark-proof" aria-label={`查看 ${task.name} 的水印截图`} title={proof.text} onClick={onInspect}>{failedProof===proof.key?<span>水印截图暂不可用</span>:<img src={`/api/watermarks/evidence?vid=${encodeURIComponent(task.vid)}&key=${encodeURIComponent(proof.key)}`} alt="" loading="lazy" data-private-media draggable={false} onError={()=>setFailedProof(proof.key)}/>}</WorkspaceButton>}</div>
    <div className="review-summary-main"><WorkspaceButton variant="ghost" className="review-summary-title" aria-label={`核对 ${task.name}`} title="查看详情" onClick={onInspect}><span className="ws-row-name">{task.name}</span><ChevronRight size={14}/></WorkspaceButton><div className="review-summary-meta"><span>{(task.size/2**30).toFixed(2)} GB</span><span data-warning={summary.conflict||!!task.manual}>{summary.conflict&&<AlertTriangle size={12}/>} {summary.status}</span></div><WorkspacePath value={task.path} compact/></div>
    <div className="review-summary-facts">
      <div className="review-watermarks" role="group" aria-label="识别到的水印文字"><ScanText size={14} aria-hidden="true"/><div>{marks.length?marks.map(mark=><div key={mark.key} className="review-watermark"><code>{mark.text}</code><span className="review-watermark-meta" data-confirmed={mark.confirmed}>{mark.label}{mark.frames!==null?` · ${mark.frames} 帧`:''}{mark.time!==null?` · ${mark.time.toFixed(1)} 秒`:''}</span></div>):<span className="ws-muted">{task.data.watermark?.status==='error'?'水印读取失败':task.data.watermark?'本轮未检出重复文字':'水印依据未记录'}</span>}{summary.marks.length>marks.length&&<span className="ws-muted">另有 {summary.marks.length-marks.length} 条文字线索</span>}</div></div>
      <div className="review-candidates" role="group" aria-label="人物候选依据">{candidates.length?candidates.map(candidate=><div key={candidate.name} className="review-candidate"><strong>{candidate.name}</strong><ul>{candidate.reasons.slice(0,3).map(reason=><li key={reason.kind+reason.text} data-confirmed={reason.confirmed}>{reason.text}</li>)}{candidate.reasons.length>3&&<li className="ws-muted">另有 {candidate.reasons.length-3} 条依据，详情可查看</li>}</ul></div>):<p className="ws-muted">暂无人物候选{task.data.face?.unknown_group?'，仅有未命名相似组':''}</p>}{summary.candidates.length>candidates.length&&<WorkspaceButton variant="ghost" onClick={onInspect}>另 {summary.candidates.length-candidates.length} 个候选，查看冲突</WorkspaceButton>}</div>
      {summary.warning&&<p className="review-summary-warning" title={summary.warning}>{summary.warning}</p>}{task.note&&<p className="review-summary-note" title={task.note}>{task.note}</p>}
    </div>
  </Card>
}

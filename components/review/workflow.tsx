'use client'
import Link from 'next/link'
import { ArrowRight, Check, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { WorkspaceButton,WorkspaceCard } from '@/components/workbench/inner'

export function ReviewFlow({ step = 1 }: { step?: 1 | 2 | 3 }) {
  return <div className="space-y-2 rounded-xl border bg-card p-3 sm:p-4">
    <ol aria-label="归档流程" className="flex flex-wrap items-center gap-x-3 gap-y-2 text-xs sm:text-sm">
      {[{ n: 1, label: '人工确认', href: '/review' }, { n: 2, label: '待归档', href: '/integration' }, { n: 3, label: '已归档', href: '/integration?view=history' }].map((s,i) => <li key={s.n} className="flex items-center gap-3"><a href={s.href} aria-current={step === s.n ? 'step' : undefined} className={`flex items-center gap-2 rounded px-1 py-1 ${step === s.n ? 'font-semibold text-foreground' : 'text-muted-foreground'}`}><span className={`flex h-6 w-6 items-center justify-center rounded-full border ${step === s.n ? 'border-foreground bg-foreground text-background' : ''}`}>{step > s.n ? <Check className="h-3 w-3" /> : s.n}</span>{s.label}</a>{i < 2 && <ArrowRight className="h-3 w-3 text-muted-foreground" />}</li>)}
    </ol>
    <p className="text-xs leading-relaxed text-muted-foreground">{step === 1 ? '现在：核对视频属于谁。确认后进入待归档，文件暂不移动。' : step === 2 ? '现在：检查已确认的视频和目标路径。执行归档才移动文件；也可以撤销，退回上一步。' : '这里是已经完成归档的视频，显示实际存放位置。'}</p>
  </div>
}
export function QueueReceipt({ count, creator, onUndo, busy = false }: { count: number; creator: string; onUndo: () => void; busy?: boolean }) {
  return <WorkspaceCard className="ws-result" role="status"><Check size={17}/><div className="min-w-0 flex-1"><strong>已确认 {count} 个视频属于 {creator}</strong><span>已加入待归档，文件暂未移动。</span></div><WorkspaceButton variant="ghost" onClick={onUndo} disabled={busy}><Undo2 size={14}/>撤销这次</WorkspaceButton><Button asChild size="sm"><Link href="/integration">查看待归档<ArrowRight size={14}/></Link></Button></WorkspaceCard>
}

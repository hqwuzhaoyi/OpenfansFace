'use client'
import { useState } from 'react'
import { api } from '@/lib/face-api'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ResponsiveDialogContent } from './responsive-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { PathText, VideoPlayer } from './video'
import type { ReviewVideo } from '@/lib/review-ui'
import type { ArchiveResolution } from '@/lib/review-logic'

export interface ConflictVideo extends ReviewVideo {
  src: string | null; dst: string | null; original_dst?: string; suggested_name?: string
  size: number; dst_size?: number; original_size?: number; resolution?: ArchiveResolution
}
const sizeText = (n?: number) => n === undefined ? '未读取' : `${(n / 2**20).toFixed(2)} MiB`

export function ArchiveResolutionDialog({ rows, version, onClose, onSaved, fallbackFocus }: {
  rows: ConflictVideo[]; version: string; onClose: () => void; onSaved: (vids: string[]) => Promise<void>
  fallbackFocus?: React.RefObject<HTMLElement | null>
}) {
  const [trigger] = useState(() => typeof document !== 'undefined' && document.activeElement instanceof HTMLElement ? document.activeElement : null)
  const [kind, setKind] = useState(''), [name, setName] = useState(rows[0]?.resolution?.kind === 'rename' ? rows[0].resolution.name : rows[0]?.suggested_name || '')
  const [saving, setSaving] = useState(false), [error, setError] = useState('')
  const [playing, setPlaying] = useState<ReviewVideo | null>(null)
  const canStage = rows.every(r => r.original_size === r.size && r.size > 0)
  const save = async () => {
    if (saving || !kind) return
    setSaving(true); setError('')
    try {
      await api('/api/plan/resolve', { method: 'POST', body: JSON.stringify({ vids: rows.map(r=>r.vid), version, kind, ...(kind === 'rename' && rows.length === 1 ? {name} : {}) }) })
      await onSaved(rows.map(r=>r.vid)); onClose()
    } catch(e) { setError((e as Error).message) } finally { setSaving(false) }
  }
  return <><Dialog open onOpenChange={open=>!open&&!saving&&onClose()}>
    <ResponsiveDialogContent open onCloseAutoFocus={event => { event.preventDefault(); const target = trigger; requestAnimationFrame(() => { if (target?.isConnected && !target.matches(':disabled')) target.focus(); else fallbackFocus?.current?.focus() }) }} className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl sm:p-0" onInteractOutside={e=>saving&&e.preventDefault()} onEscapeKeyDown={e=>saving&&e.preventDefault()} showCloseButton={!saving}>
      <DialogHeader className="shrink-0 border-b p-4 pr-14 text-left"><DialogTitle>处理同名文件 · {rows.length} 个</DialogTitle><DialogDescription>先选择处理方式，再回待归档执行。本步只保存选择，文件还不会移动。</DialogDescription></DialogHeader>
      <div data-sheet-scroll className="min-h-0 flex-1 space-y-5 overflow-y-auto p-4">
        <details className="rounded-xl border p-3"><summary className="min-h-11 cursor-pointer break-all text-sm font-medium">{rows.length===1?rows[0].name:`核对这 ${rows.length} 个视频的路径和内容`}<span className="mt-1 block text-xs font-normal text-muted-foreground">点开查看源文件和目标文件，也可以分别播放。</span></summary><div className="mt-3 space-y-3">{rows.map(r=><details key={r.vid} open={rows.length === 1} className="rounded-xl border p-3">
          <summary className="min-h-11 cursor-pointer break-all text-sm font-medium">{r.name} · 源 {sizeText(r.size)} / 目标 {sizeText(r.original_size)}</summary>
          <div className="mt-3 space-y-4"><div className="space-y-2"><PathText value={r.src || r.vid} label="源文件（待归档）"/><Button className="min-h-11" size="sm" variant="outline" onClick={()=>setPlaying({...r,path:r.src||r.vid,name:`源视频 · ${r.name}`})}>播放源视频</Button></div>
            <div className="space-y-2"><PathText value={r.original_dst || r.dst || ''} label="目标目录中已有文件"/><Button className="min-h-11" size="sm" variant="outline" onClick={()=>setPlaying({...r,path:r.original_dst||r.dst||r.vid,thumb:null,name:`目标视频 · ${r.name}`})}>播放目标视频</Button></div></div>
        </details>)}</div></details>
        <fieldset disabled={saving} className="space-y-3"><legend className="mb-3 text-sm font-semibold">这批文件要怎么处理？</legend>
          <label className={`flex min-h-11 cursor-pointer items-start gap-3 rounded-xl border p-4 ${kind==='rename'?'border-white/40 bg-muted/40':''}`}><input className="mt-1 h-4 w-4 shrink-0" type="radio" name="resolution" value="rename" checked={kind==='rename'} onChange={()=>setKind('rename')}/><span className="space-y-1"><span className="block text-sm font-medium">保留两份，给源视频换个文件名</span><span className="block text-xs leading-relaxed text-muted-foreground">目标视频保留。源视频归档时使用新名字，放到同一个人物目录。</span></span></label>
          {kind==='rename'&&<div className="space-y-2 rounded-lg border bg-muted/20 p-3">{rows.length===1?<label className="block space-y-2 text-sm"><span>归档后的文件名（保留扩展名）</span><Input className="h-11 text-base" value={name} onChange={e=>setName(e.target.value)}/></label>:rows.map(r=><p key={r.vid} className="break-all text-xs">{r.name} → {r.suggested_name}</p>)}<p className="text-xs text-muted-foreground">下一步：保存选择 → 检查新路径 → 点击归档。</p></div>}
          <label className={`flex min-h-11 items-start gap-3 rounded-xl border p-4 ${canStage?'cursor-pointer':'opacity-50'} ${kind==='stage-duplicate'?'border-white/40 bg-muted/40':''}`}><input className="mt-1 h-4 w-4 shrink-0" type="radio" name="resolution" value="stage-duplicate" checked={kind==='stage-duplicate'} disabled={!canStage} onChange={()=>setKind('stage-duplicate')}/><span className="space-y-1"><span className="block text-sm font-medium">保留目标视频，将重复源文件移入暂存</span><span className="block text-xs leading-relaxed text-muted-foreground">{canStage?'执行时完整校验 SHA-256。只有内容完全相同才暂存源文件；不同则停止并保留任务。源文件会保留在暂存目录，路径写进归档记录。':'源文件和目标体积不同，不能作为重复件处理。'}</span></span></label>
          <label className={`flex min-h-11 cursor-pointer items-start gap-3 rounded-xl border p-4 ${kind==='clear'?'border-white/40 bg-muted/40':''}`}><input className="mt-1 h-4 w-4 shrink-0" type="radio" name="resolution" value="clear" checked={kind==='clear'} onChange={()=>setKind('clear')}/><span className="space-y-1"><span className="block text-sm font-medium">暂不处理，留在这一波</span><span className="block text-xs leading-relaxed text-muted-foreground">保留人物归名和所有文件，下次再选择处理方式。</span></span></label>
        </fieldset>
        {error&&<p role="alert" className="rounded-lg border border-amber-500/30 p-3 text-sm text-amber-300">{error}</p>}
      </div>
      <div className="mobile-bottom-actions flex shrink-0 flex-col gap-2 border-t bg-background p-3 sm:flex-row sm:justify-end"><Button className="min-h-11" variant="outline" disabled={saving} onClick={onClose}>返回检查</Button><Button className="min-h-11" disabled={saving||!kind||(kind==='rename'&&rows.length===1&&!name.trim())} onClick={save}>{saving?'正在保存…':'保存选择，返回待归档'}</Button></div>
    </ResponsiveDialogContent>
  </Dialog><VideoPlayer video={playing} onClose={()=>setPlaying(null)}/></>
}

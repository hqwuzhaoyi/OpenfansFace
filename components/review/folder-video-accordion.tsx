'use client'
import { Accordion } from 'radix-ui'
import { ChevronDown, Folder } from 'lucide-react'
import { videoGroups, type ReviewVideo } from '@/lib/review-ui'
import { VideoRow } from './video'

export function FolderVideoAccordion({videos,selected,onToggle,onPlay,disabled,value,onValueChange}:{
  videos:ReviewVideo[];selected:Set<string>;onToggle:(ids:string[])=>void
  onPlay:(video:ReviewVideo)=>void;disabled?:boolean;value:string[];onValueChange:(folders:string[])=>void
}) {
  return <Accordion.Root type="multiple" value={value} onValueChange={onValueChange} className="space-y-2">
    {videoGroups(videos).map(([dir,rows])=>{
      const count=rows.filter(v=>selected.has(v.vid)).length
      return <Accordion.Item key={dir} value={dir} data-folder={dir} className={`min-w-0 overflow-hidden rounded-xl border ${count?'border-white/30 bg-white/[0.025]':'bg-card'}`}>
        <div className="flex items-start">
          <label className="mt-1 flex min-h-11 w-11 shrink-0 cursor-pointer items-center justify-center">
            <input type="checkbox" checked={count===rows.length} disabled={disabled}
              ref={el=>{if(el)el.indeterminate=count>0&&count<rows.length}}
              onChange={()=>onToggle(rows.map(v=>v.vid))} aria-label={`选择文件夹 ${dir}`}
              className="h-4 w-4 accent-white" />
          </label>
          <Accordion.Header className="min-w-0 flex-1">
            <Accordion.Trigger aria-label={`展开或收起文件夹 ${dir}`} className="group flex min-h-14 w-full items-start gap-2 rounded-lg p-3 pl-0 text-left active:bg-accent/50 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
              <Folder className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 space-y-1"><span className="block break-all text-xs leading-relaxed">{dir}</span><span className={`block text-xs ${count?'text-foreground':'text-muted-foreground'}`}>{rows.length} 个视频 · 已选 {count}/{rows.length}</span></span>
              <ChevronDown className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground group-data-[state=open]:rotate-180" />
            </Accordion.Trigger>
          </Accordion.Header>
        </div>
        <Accordion.Content data-folder-content className="space-y-2 px-3 pb-3">
          {rows.map(video=><VideoRow key={video.vid} video={video} selected={selected.has(video.vid)} onToggle={()=>onToggle([video.vid])} onPlay={onPlay} disabled={disabled}/>)}
        </Accordion.Content>
      </Accordion.Item>
    })}
  </Accordion.Root>
}

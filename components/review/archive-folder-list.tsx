'use client'

import { useId } from 'react'
import { ChevronDown, ChevronRight, FileVideo, Folder } from 'lucide-react'
import { groupSourceFolders } from '@/lib/integration-view'
import { fileName } from '@/lib/review-ui'
import './archive-workbench.css'

export interface ArchiveListItem {
  vid: string
  name: string
  src: string | null
  dst?: string | null
  thumb: string | null
  creator: string
  status: string
  statusLabel: string
  size_gb?: number
}

export function ArchiveCheckbox({ checked, mixed, disabled, label, onChange }: {
  checked: boolean; mixed?: boolean; disabled?: boolean; label: string; onChange: () => void
}) {
  return <label className="archive-checkboxTarget">
    <input type="checkbox" aria-label={label} checked={checked} disabled={disabled}
      ref={el => { if (el) el.indeterminate = !!mixed }} onChange={onChange} className="archive-checkbox" />
  </label>
}

export function ArchiveFolderList({ items, selected, active, collapsed, disabled, history, onToggle, onOpen, onCollapse }: {
  items: ArchiveListItem[]; selected: Set<string>; active?: string; collapsed: Set<string>
  disabled: boolean; history: boolean; onToggle: (ids: string[]) => void
  onOpen: (vid: string) => void; onCollapse: (path: string) => void
}) {
  const id = useId()
  return <>{groupSourceFolders(items).map((folder, index) => {
    const count = folder.items.filter(item => selected.has(item.vid)).length
    const open = !collapsed.has(folder.path)
    const contentId = `${id}-folder-${index}`
    return <section key={folder.path} className="archive-folder" data-folder={folder.path}>
      <div className="archive-folderHeader">
        {!history && <ArchiveCheckbox checked={count === folder.items.length} mixed={count > 0 && count < folder.items.length}
          disabled={disabled} label={`选择文件夹 ${folder.path} 当前显示的 ${folder.items.length} 项`} onChange={() => onToggle(folder.items.map(item => item.vid))} />}
        <button className="archive-folderToggle" aria-expanded={open} aria-controls={contentId} onClick={() => onCollapse(folder.path)} title={folder.path}>
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}<Folder size={15} />
          <span className="archive-folderName"><strong>{fileName(folder.path)}</strong><small>{folder.path}</small></span>
          <span className="archive-folderCount">{count ? `${count} / ` : ''}{folder.items.length}</span>
          <span className="sr-only">{count ? `已选 ${count} 个视频` : ''}</span>
        </button>
      </div>
      <div id={contentId} hidden={!open}>
        {folder.items.map(item => <div key={item.vid} data-vid={item.vid} data-active={active === item.vid} data-selected={selected.has(item.vid)} className="archive-row">
          {!history && <ArchiveCheckbox checked={selected.has(item.vid)} disabled={disabled} label={`选择 ${item.name}`} onChange={() => onToggle([item.vid])} />}
          <button className="archive-rowOpen" aria-current={active === item.vid ? 'true' : undefined} onClick={() => onOpen(item.vid)} title={item.name}>
            <span className="archive-rowThumbnail">{item.thumb ? <img data-private-media draggable={false} src={item.thumb} alt="" loading="lazy" /> : <FileVideo size={18} />}</span>
            <span className="archive-rowText"><strong>{item.name}</strong><small>{item.creator}{item.size_gb !== undefined && <> · {item.size_gb.toFixed(2)} GB</>}</small></span>
            <span className={item.status === 'ok' || history ? 'archive-good' : 'archive-warning'} title={item.statusLabel}>
              {item.status === 'ok' || history ? <><span className="archive-dot" /><span className="sr-only">{item.statusLabel}</span></> : <span className="archive-rowStatus">{item.statusLabel}</span>}
            </span>
          </button>
        </div>)}
      </div>
    </section>
  })}</>
}

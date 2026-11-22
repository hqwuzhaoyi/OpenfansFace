'use client'

import { useId } from 'react'
import { ChevronDown, ChevronRight, FileVideo, Folder } from 'lucide-react'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { groupSourceFolders } from '@/lib/integration-view'
import { lockBadgeStatus, resolveEntityLock, type ScopeLockInfo } from '@/lib/locks-model'
import { fileName } from '@/lib/review-ui'
import './archive-workbench.css'

export interface ArchiveListItem {
  rowKey?: string
  vid: string
  name: string
  src: string | null
  dst?: string | null
  thumb: string | null
  creator: string
  status: string
  statusLabel: string
  size_gb?: number
  source?: string
  undone?: boolean
}

export function ArchiveCheckbox({ checked, mixed, disabled, label, onChange }: {
  checked: boolean; mixed?: boolean; disabled?: boolean; label: string; onChange: () => void
}) {
  return <label className="archive-checkboxTarget">
    <input type="checkbox" aria-label={label} checked={checked} disabled={disabled}
      ref={el => { if (el) el.indeterminate = !!mixed }} onChange={onChange} className="archive-checkbox" />
  </label>
}

export function ArchiveFolderList({ items, selected, active, collapsed, disabled, history, lockedPeople, lockedVids, onToggle, onOpen, onCollapse }: {
  items: ArchiveListItem[]; selected: Set<string>; active?: string; collapsed: Set<string>
  disabled: boolean; history: boolean
  lockedPeople?: ReadonlyMap<string, ScopeLockInfo>
  lockedVids?: ReadonlyMap<string, ScopeLockInfo>
  onToggle: (ids: string[]) => void
  onOpen: (vid: string) => void; onCollapse: (path: string) => void
}) {
  const id = useId()
  return <>{groupSourceFolders(items).map((folder, index) => {
    const itemLock = (item: ArchiveListItem) => resolveEntityLock(lockedVids?.get(item.vid), lockedPeople?.get(item.creator))
    const unlockedVids = folder.items.filter(item => !itemLock(item)).map(item => item.vid)
    const count = folder.items.filter(item => selected.has(item.vid)).length
    const unlockedCount = unlockedVids.filter(vid => selected.has(vid)).length
    const open = !collapsed.has(folder.path)
    const contentId = `${id}-folder-${index}`
    return <section key={folder.path} className="archive-folder" data-folder={folder.path}>
      <div className="archive-folderHeader">
        {!history && <ArchiveCheckbox checked={unlockedVids.length > 0 && unlockedCount === unlockedVids.length} mixed={unlockedCount > 0 && unlockedCount < unlockedVids.length}
          disabled={disabled || unlockedVids.length === 0} label={`选择文件夹 ${folder.path} 当前显示的 ${folder.items.length} 项`} onChange={() => onToggle(unlockedVids)} />}
        <button className="archive-folderToggle" aria-expanded={open} aria-controls={contentId} onClick={() => onCollapse(folder.path)} title={folder.path}>
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}<Folder size={15} />
          <span className="archive-folderName"><strong>{fileName(folder.path)}</strong><small>{folder.path}</small></span>
          <span className="archive-folderCount">{count ? `${count} / ` : ''}{folder.items.length}</span>
          <span className="sr-only">{count ? `已选 ${count} 个视频` : ''}</span>
        </button>
      </div>
      <div id={contentId} hidden={!open}>
        {folder.items.map(item => {
          const itemKey = item.rowKey || item.vid
          const rowLock = itemLock(item)
          return <div key={itemKey} data-vid={item.vid} data-active={active === itemKey} data-selected={selected.has(item.vid)} data-undone={item.undone || undefined} data-locked={Boolean(rowLock) || undefined} className="archive-row">
          {!history && <ArchiveCheckbox checked={selected.has(item.vid)} disabled={disabled || Boolean(rowLock)} label={`选择 ${item.name}`} onChange={() => { if (!rowLock) onToggle([item.vid]) }} />}
          <button className="archive-rowOpen" aria-current={active === itemKey ? 'true' : undefined} onClick={() => onOpen(itemKey)} title={rowLock ? `${item.name} · ${rowLock.label}` : item.name}>
            <span className="archive-rowThumbnail">{item.thumb ? <img data-private-media draggable={false} src={item.thumb} alt="" loading="lazy" /> : <FileVideo size={18} />}</span>
            <span className="archive-rowText"><strong>{item.name}</strong><small className="archive-rowMeta"><span>{item.creator}{item.size_gb !== undefined && <> · {item.size_gb.toFixed(2)} GB</>}</span>{item.source === 'auto-watermark' && <span className="archive-autoBadge">自动水印</span>}</small></span>
            {rowLock ? <AnimatedBadge size="sm" status={lockBadgeStatus(rowLock.status)} className="archive-lockBadge">{rowLock.label}</AnimatedBadge> : item.undone ? <span className="archive-undoneBadge">已撤回</span> : <span className={item.status === 'ok' || history ? 'archive-good' : 'archive-warning'} title={item.statusLabel}>
              {item.status === 'ok' || history ? <><span className="archive-dot" /><span className="sr-only">{item.statusLabel}</span></> : <span className="archive-rowStatus">{item.statusLabel}</span>}
            </span>}
          </button>
        </div>
        })}
      </div>
    </section>
  })}</>
}

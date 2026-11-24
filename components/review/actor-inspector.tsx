'use client'
import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowLeftRight,
  BookmarkPlus,
  Check,
  ChevronDown,
  ChevronUp,
  Eye,
  FileText,
  Folder,
  Link2,
  Maximize2,
  Pencil,
  Play,
  Save,
  ScanFace,
  Sparkles,
  UserPlus,
  X,
} from 'lucide-react'
import {
  confirmedWatermarkKeys,
  findSimilarExistingPeople,
  normPersonText,
  rankPersonChoices,
  watermarkMatchesPerson,
  type PersonChoice,
  type PersonNameHint,
} from '@/lib/person-picker-model'
import { isDestructivePersonAction, lockBadgeStatus, type ScopeLockInfo } from '@/lib/locks-model'
import type { FaceMatchClue, ProcessingTask } from '@/lib/processing-model'
import { reviewListSummary } from '@/lib/review-list-summary'
import type { ReviewVideo } from '@/lib/review-ui'
import { usePersonReferences } from './reference-panel'
import { VideoCover } from './video-cover'
import { PrivateImage } from '@/components/private-media'
import { ExternalProfiles } from '@/components/persons/external-profiles'
import { Card } from '@/components/beui/motion/card'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { Avatar, AvatarGroup } from '@/components/beui/motion/avatar-group'
import { ActionSwapIcon } from '@/components/beui/motion/action-swap'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxLabel,
  ComboboxList,
  ComboboxSeparator,
  ComboboxTrigger,
} from '@/components/beui/motion/combobox'
import {
  ImageViewer,
  ImageViewerCaption,
  ImageViewerClose,
  ImageViewerContent,
  ImageViewerCounter,
  ImageViewerNext,
  ImageViewerPrevious,
  ImageViewerThumbnail,
  useImageViewer,
  type LightboxImage,
} from '@/components/beui/motion/image-viewer'
import {
  WorkspaceButton as Button,
  WorkspaceField,
  WorkspaceInput,
  WorkspaceMedia,
  WorkspaceMessage,
  WorkspacePath,
  WorkspaceSection,
  WorkspaceTextarea,
} from '@/components/workbench/inner'
import { Kbd } from '@/components/ui/kbd'

export interface ActorInspectorHandle {
  openSwitcher: () => void
}

const COMBOBOX_LABELS = {
  valuePlaceholder: '搜索或选择人物',
  inputLabel: '搜索或切换人物',
  inputPlaceholder: '搜索人物或别名…',
  listLabel: '人物列表',
  empty: '没有匹配的人物',
}

const VIEWER_LABELS = {
  label: '人像与对照核对',
  open: '放大查看 {alt}',
  counter: '{current} / {total}',
  close: '关闭预览',
  previous: '上一张',
  next: '下一张',
  loadError: '图片暂不可用。',
  zoomIn: '放大',
  zoomOut: '还原缩放',
  panHelp: '放大后可拖拽查看细节，或保持焦点并使用方向键移动。',
}

function toThumbSrc(raw?: string | null): string | null {
  if (!raw) return null
  if (raw.startsWith('/thumb?') || raw.startsWith('http://') || raw.startsWith('https://')) return raw
  return `/thumb?p=${encodeURIComponent(raw)}`
}

function formatTimestamp(sec?: number): string {
  if (typeof sec !== 'number' || !Number.isFinite(sec) || sec < 0) return ''
  const total = Math.floor(sec)
  const mins = Math.floor(total / 60)
  const secs = total % 60
  return `${mins}:${String(secs).padStart(2, '0')}`
}

function validateNewPersonName(raw: string): string | null {
  const trimmed = raw.trim()
  if (!trimmed) return null
  if (trimmed.length > 120) return '名称不超过 120 个字符'
  if (/[\\/\x00-\x1f]/.test(raw) || trimmed === '.' || trimmed === '..') {
    return '不能包含路径分隔符 / 或 \\'
  }
  return null
}

function InspectorViewerContent({
  suggestionMap,
  catalogMap,
  currentValue,
  disabled,
  onSelectPerson,
  onComparePerson,
  refVideos,
  refOwner,
}: {
  suggestionMap: Map<string, PersonChoice>
  catalogMap: Map<string, PersonChoice>
  currentValue: string
  disabled?: boolean
  onSelectPerson: (name: string) => void
  onComparePerson: (name: string, refIndex?: number) => void
  refVideos: ReviewVideo[]
  refOwner: string | null
}) {
  const { image, close } = useImageViewer()
  const id = image?.id || ''
  const isMatch = id.startsWith('match:')
  const isSample = id.startsWith('sample:')
  const isPerson = id.startsWith('person:')
  const isRef = id.startsWith('ref:')
  const personName = isPerson ? id.slice('person:'.length) : null
  const refVid = isRef ? id.slice('ref:'.length) : null
  const refIndex = refVid ? refVideos.findIndex(v => v.vid === refVid) : -1
  const suggested = personName ? suggestionMap.get(personName) : undefined
  const personObj = personName ? catalogMap.get(personName) || suggested : undefined
  const canCompare = Boolean(personName && (catalogMap.size === 0 || catalogMap.has(personName)))
  const kindBadge = isMatch
    ? '人脸匹配'
    : isSample
      ? '人脸采样'
      : isRef
        ? '对照'
        : suggested?.sources?.includes('face')
          ? '人脸候选'
          : suggested?.tier === 'related'
            ? '关联'
            : suggested
              ? '候选'
              : isPerson
                ? '人物'
                : null
  const captionMeta =
    suggested?.match_reason ||
    (personObj?.aliases?.length ? `别名：${personObj.aliases.slice(0, 2).join(' · ')}` : '')

  return (
    <ImageViewerContent
      header={
        <>
          <div className="flex min-w-0 items-center gap-2">
            <ImageViewerCounter />
            {kindBadge && (
              <span className="rounded-full bg-white/15 px-2 py-0.5 text-[11px] font-medium text-white">
                {kindBadge}
              </span>
            )}
          </div>
          <ImageViewerClose />
        </>
      }
    >
      <ImageViewerPrevious />
      <div
        data-lightbox-content=""
        className="flex min-w-0 flex-1 items-center justify-center gap-3 px-2"
      >
        <ImageViewerCaption className="min-w-0 truncate">
          {personName ? (
            <span className="inline-flex max-w-full min-w-0 items-baseline gap-2">
              <strong className="truncate font-semibold text-white">{personName}</strong>
              {captionMeta && (
                <span className="hidden truncate text-xs text-white/70 sm:inline">
                  {captionMeta}
                </span>
              )}
            </span>
          ) : undefined}
        </ImageViewerCaption>
        {personName && (
          <div className="flex shrink-0 items-center gap-1.5">
            <button
              type="button"
              disabled={disabled}
              onClick={() => {
                onSelectPerson(personName)
                close()
              }}
              className="inline-flex h-8 items-center gap-1 rounded-full bg-white px-3 text-xs font-medium text-neutral-950 transition-colors hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white disabled:opacity-40"
            >
              <Check size={13} aria-hidden="true" />
              {currentValue === personName ? '已选' : '选此人'}
            </button>
            {canCompare && (
              <button
                type="button"
                disabled={disabled}
                onClick={() => {
                  onComparePerson(personName, 0)
                  close()
                }}
                className="inline-flex h-8 items-center gap-1 rounded-full bg-white/15 px-3 text-xs font-medium text-white transition-colors hover:bg-white/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white disabled:opacity-40"
              >
                <Eye size={13} aria-hidden="true" />
                对照
              </button>
            )}
          </div>
        )}
        {refIndex >= 0 && refOwner && (
          <button
            type="button"
            onClick={() => {
              onComparePerson(refOwner, refIndex)
              close()
            }}
            className="inline-flex h-8 shrink-0 items-center gap-1 rounded-full bg-white px-3 text-xs font-medium text-neutral-950 transition-colors hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white"
          >
            <Play size={12} aria-hidden="true" />
            播放对照
          </button>
        )}
      </div>
      <ImageViewerNext />
    </ImageViewerContent>
  )
}

export interface ActorInspectorProps {
  task?: ProcessingTask
  hideMedia?: boolean
  poster?: string
  note: string
  onNoteChange: (note: string) => void
  onSaveNote: () => void
  onSource: (key: string) => Promise<boolean>
  busy: boolean
  locked?: boolean
  lockedPeople?: ReadonlyMap<string, ScopeLockInfo>
  lockedVids?: ReadonlyMap<string, ScopeLockInfo>
  people: PersonChoice[]
  peopleLoading?: boolean
  peopleError?: string
  onRetryPeople?: () => void
  existingPeople?: ReadonlySet<string>
  suggestions: PersonChoice[]
  sampleFaces: string[]
  nameHints: PersonNameHint[]
  creator: string
  onChoose: (name: string) => void
  search?: string
  onSearchChange?: (value: string) => void
  actionTargets: ProcessingTask[]
  commonMarks: NonNullable<NonNullable<ProcessingTask['data']['watermark']>['candidates']>
  watermarkKey: string
  onWatermarkKeyChange: (key: string) => void
  hiddenCount?: number
  groupLabel?: string
  selectionPreviewVid?: string | null
  onPreviewTarget?: (task: ProcessingTask) => void
  onToggleTarget?: (task: ProcessingTask) => void
  activeReferenceName?: string | null
  stageTab?: 'current' | 'reference'
  stageRefData?: { name: string; videos: ReviewVideo[]; total: number; error?: string } | null
  stageRefLoading?: boolean
  onRetryStageRef?: () => void
  onReference: (name: string, refIndex?: number) => void
}

export const ActorInspector = forwardRef<ActorInspectorHandle, ActorInspectorProps>(
  function ActorInspector(
    {
      task,
      hideMedia = false,
      poster,
      note,
      onNoteChange,
      onSaveNote,
      onSource,
      busy,
      locked = false,
      lockedPeople,
      lockedVids,
      people,
      peopleLoading = false,
      peopleError = '',
      onRetryPeople,
      existingPeople,
      suggestions,
      sampleFaces,
      nameHints,
      creator,
      onChoose,
      search,
      onSearchChange,
      actionTargets,
      commonMarks,
      watermarkKey,
      onWatermarkKeyChange,
      hiddenCount = 0,
      groupLabel,
      selectionPreviewVid,
      onPreviewTarget,
      onToggleTarget,
      activeReferenceName = null,
      stageTab = 'current',
      stageRefData = null,
      stageRefLoading = false,
      onRetryStageRef,
      onReference,
    },
    ref,
  ) {
    const [localQuery, setLocalQuery] = useState('')
    const [comboOpen, setComboOpen] = useState(false)
    const [creating, setCreating] = useState(false)
    const [scopeOpen, setScopeOpen] = useState(false)
    const [sourceConfirm, setSourceConfirm] = useState<string | null>(null)
    const [viewerId, setViewerId] = useState<string | null>(null)

    const previousCreator = useRef(creator)
    const comboInputRef = useRef<HTMLInputElement>(null)
    const newInputRef = useRef<HTMLInputElement>(null)

    const query = search ?? localQuery
    const setQuery = onSearchChange ?? setLocalQuery

    useImperativeHandle(ref, () => ({
      openSwitcher: () => {
        setCreating(false)
        setComboOpen(true)
        requestAnimationFrame(() => comboInputRef.current?.focus({ preventScroll: true }))
      },
    }))

    const suggestionMap = useMemo(() => new Map(suggestions.map(s => [s.name, s])), [suggestions])
    const catalogMap = useMemo(() => new Map(people.map(p => [p.name, p])), [people])
    const catalogReady = people.length > 0

    const trimmedCreator = creator.trim()
    const focusActor = trimmedCreator || suggestions[0]?.name || ''
    const isFocusExisting = Boolean(
      focusActor && (existingPeople ? !existingPeople.size || existingPeople.has(focusActor) : !catalogReady || catalogMap.has(focusActor)),
    )

    // Reuse stageRefData when focusActor matches the middle stage reference owner; otherwise load on demand.
    const reuseStageRef = Boolean(focusActor && stageRefData && stageRefData.name === focusActor)
    const shouldFetchLocalRef = Boolean(
      !creating && focusActor && isFocusExisting && !reuseStageRef && !(focusActor === activeReferenceName && stageRefLoading),
    )
    const {
      current: localRefData,
      loading: localRefLoading,
      retry: retryLocalRef,
    } = usePersonReferences(shouldFetchLocalRef ? focusActor : null)

    const refData = reuseStageRef ? stageRefData : localRefData
    const refLoading = reuseStageRef ? stageRefLoading : focusActor === activeReferenceName ? stageRefLoading : localRefLoading
    const retryRef = reuseStageRef || focusActor === activeReferenceName ? onRetryStageRef : retryLocalRef

    const combined = useMemo(() => {
      return [
        ...new Map(
          [
            ...suggestions.map(s => {
              const existing = catalogMap.get(s.name)
              return {
                ...existing,
                ...s,
                rep_thumb: s.rep_thumb || existing?.rep_thumb || null,
                aliases: s.aliases?.length ? s.aliases : existing?.aliases,
              }
            }),
            ...people.filter(p => p.recent_at),
            ...people,
          ].map(p => [p.name, p]),
        ).values(),
      ]
    }, [suggestions, catalogMap, people])

    const trimmedQuery = query.trim()
    const rankedChoices = useMemo(() => rankPersonChoices(combined, query), [combined, query])

    // Partition Combobox items without duplicates so active highlight stays deterministic and lightweight.
    const { recItems, recentItems, allItems } = useMemo(() => {
      if (trimmedQuery) {
        return { recItems: [] as PersonChoice[], recentItems: [] as PersonChoice[], allItems: rankedChoices.slice(0, 60) }
      }
      const rec = rankedChoices.filter(p => suggestionMap.has(p.name))
      const recSet = new Set(rec.map(p => p.name))
      const recent = rankedChoices.filter(p => !recSet.has(p.name) && Boolean(p.recent_at)).slice(0, 8)
      const recentSet = new Set(recent.map(p => p.name))
      const quota = Math.max(20, 60 - rec.length - recent.length)
      const rest = rankedChoices.filter(p => !recSet.has(p.name) && !recentSet.has(p.name)).slice(0, quota)
      // Ensure an explicitly selected actor outside the initial 60 window stays registered as selected in Combobox.
      if (
        trimmedCreator &&
        !recSet.has(trimmedCreator) &&
        !recentSet.has(trimmedCreator) &&
        !rest.some(p => p.name === trimmedCreator)
      ) {
        const picked = catalogMap.get(trimmedCreator)
        if (picked) rest.unshift(picked)
      }
      return { recItems: rec, recentItems: recent, allItems: rest }
    }, [trimmedQuery, rankedChoices, suggestionMap, trimmedCreator, catalogMap])

    const canQuickCreateFromSearch =
      !creating &&
      Boolean(trimmedQuery) &&
      !validateNewPersonName(trimmedQuery) &&
      !people.some(p => normPersonText(p.name) === normPersonText(trimmedQuery))

    const normalizedDraft = normPersonText(creator)
    const validationError = creating ? validateNewPersonName(creator) : null
    const existingExactOrAlias =
      creating && normalizedDraft && !validationError
        ? people.find(
            p =>
              normPersonText(p.name) === normalizedDraft ||
              (p.aliases || []).some(a => normPersonText(a) === normalizedDraft),
          ) || null
        : null
    const similarExisting =
      creating && normalizedDraft.length >= 2 && !existingExactOrAlias && !validationError
        ? findSimilarExistingPeople(people, trimmedCreator, 3)
        : []
    const validHints = nameHints.filter(
      h => h.name.trim() && !validateNewPersonName(h.name) && h.name.trim() !== trimmedCreator,
    )

    const faceMatches: FaceMatchClue[] = useMemo(() => {
      const raw = task?.data?.face?.matches
      if (!Array.isArray(raw)) return []
      return raw.filter((m): m is FaceMatchClue => Boolean(m && typeof m.name === 'string' && m.name))
    }, [task])

    const faceThumbs = task?.data?.face?.thumbs || []

    // Focus actor card details
    const focusSuggested = focusActor ? suggestionMap.get(focusActor) : undefined
    const focusCatalog = focusActor ? catalogMap.get(focusActor) : undefined
    const focusObj = focusActor ? focusCatalog || focusSuggested || { name: focusActor, rep_thumb: null } : null
    const focusLibraryThumb = isFocusExisting ? focusCatalog?.rep_thumb || (focusSuggested && !focusSuggested.is_new ? focusSuggested.rep_thumb : null) || null : null

    // Build unified ImageViewer gallery: matched face crops -> video samples -> actor library thumbs -> reference thumbs
    const viewerImages = useMemo(() => {
      const map = new Map<string, LightboxImage>()
      const add = (img: LightboxImage) => {
        if (img.src && !map.has(img.id)) map.set(img.id, img)
      }
      faceMatches.forEach((m, idx) => {
        const src = toThumbSrc(m.thumb)
        if (!src) return
        const scorePart = typeof m.score === 'number' ? ` · ${m.score.toFixed(3)}` : ''
        add({
          id: `match:${m.name}:${idx}`,
          src,
          alt: `${m.name} 视频人脸`,
          width: 400,
          height: 400,
          caption: `视频人脸匹配 ${m.name}${scorePart}`,
        })
      })
      sampleFaces.slice(0, 6).forEach((src, idx) => {
        add({
          id: `sample:${idx}`,
          src,
          alt: `人脸采样 ${idx + 1}`,
          width: 400,
          height: 400,
          caption: `当前视频人脸采样 ${idx + 1}`,
        })
      })
      for (const s of suggestions) {
        if (s.is_new) continue
        const thumb = catalogMap.get(s.name)?.rep_thumb || s.rep_thumb
        if (!thumb) continue
        add({
          id: `person:${s.name}`,
          src: thumb,
          alt: s.name,
          width: 400,
          height: 400,
          caption: s.name,
        })
      }
      for (const m of faceMatches) {
        const thumb = catalogMap.get(m.name)?.rep_thumb
        if (!thumb) continue
        add({
          id: `person:${m.name}`,
          src: thumb,
          alt: m.name,
          width: 400,
          height: 400,
          caption: m.name,
        })
      }
      if (isFocusExisting && focusLibraryThumb) {
        add({
          id: `person:${focusObj!.name}`,
          src: focusLibraryThumb,
          alt: focusObj!.name,
          width: 400,
          height: 400,
          caption: focusObj!.name,
        })
      }
      for (const v of (refData?.videos || []).slice(0, 6)) {
        if (!v.thumb) continue
        add({
          id: `ref:${v.vid}`,
          src: v.thumb,
          alt: v.name,
          width: 640,
          height: 400,
          caption: `${focusActor} · ${v.name}`,
        })
      }
      return [...map.values()]
    }, [faceMatches, sampleFaces, suggestions, catalogMap, focusObj, isFocusExisting, focusLibraryThumb, refData?.videos, focusActor])

    const viewerImageIds = useMemo(() => new Set(viewerImages.map(img => img.id)), [viewerImages])

    const startCreate = (initialName?: string) => {
      previousCreator.current = creator
      const seed =
        typeof initialName === 'string'
          ? initialName
          : trimmedQuery || (!isFocusExisting && trimmedCreator ? trimmedCreator : validHints[0]?.name || '')
      setComboOpen(false)
      setCreating(true)
      onChoose(seed)
      requestAnimationFrame(() => {
        newInputRef.current?.focus()
        newInputRef.current?.select()
      })
    }

    const cancelCreate = () => {
      onChoose(previousCreator.current)
      setCreating(false)
      requestAnimationFrame(() => comboInputRef.current?.focus({ preventScroll: true }))
    }

    const commitCreate = () => {
      if (!trimmedCreator || validationError) return
      if (existingExactOrAlias) onChoose(existingExactOrAlias.name)
      else onChoose(trimmedCreator)
      setCreating(false)
    }

    const handleSelectPerson = (name: string) => {
      setCreating(false)
      setComboOpen(false)
      setQuery('')
      onChoose(name)
    }

    const actorCards = useMemo(() => {
      const list = [...suggestions.slice(0, 8)]
      if (trimmedCreator && !list.some(s => s.name === trimmedCreator)) {
        const fromCatalog = catalogMap.get(trimmedCreator)
        list.unshift(
          fromCatalog || {
            name: trimmedCreator,
            rep_thumb: null,
            is_new: !catalogReady ? undefined : true,
          },
        )
      }
      return list
    }, [suggestions, trimmedCreator, catalogMap, catalogReady])

    const renderComboboxItem = (person: PersonChoice) => {
      const sug = suggestionMap.get(person.name)
      const isFace = sug?.sources?.includes('face')
      const sub =
        sug?.match_reason ||
        (person.aliases?.length ? `别名：${person.aliases.slice(0, 2).join(' · ')}` : '')
      const keywords = [
        ...(person.aliases || []),
        sug?.match_reason || '',
      ].filter(Boolean)

      return (
        <ComboboxItem
          key={person.name}
          value={person.name}
          textValue={person.name}
          keywords={keywords}
          className="py-1.5"
        >
          <span className="flex min-w-0 items-center gap-2">
            <Avatar
              name={person.name}
              src={person.rep_thumb || undefined}
              size="sm"
              className="shrink-0"
            />
            <span className="min-w-0 flex-1">
              <span className="flex min-w-0 items-center gap-1.5">
                <span className="truncate text-xs font-medium text-foreground">{person.name}</span>
                {sug && (
                  <AnimatedBadge
                    size="sm"
                    showIcon={false}
                    status={
                      sug.is_new
                        ? 'warning'
                        : isFace
                          ? 'success'
                          : sug.tier === 'related'
                            ? 'info'
                            : 'success'
                    }
                    className="h-4 px-1 text-[10px]"
                  >
                    {sug.is_new ? '新名' : isFace ? '人脸' : sug.tier === 'related' ? '关联' : '候选'}
                  </AnimatedBadge>
                )}
              </span>
              {sub && <span className="block truncate text-[11px] text-muted-foreground">{sub}</span>}
            </span>
          </span>
        </ComboboxItem>
      )
    }

    const marks = task?.data?.watermark?.candidates || []
    const commonMarkKeys = useMemo(() => new Set(commonMarks.map(c => c.key)), [commonMarks])
    const confirmedForCreator = useMemo(
      () => (trimmedCreator ? confirmedWatermarkKeys(actionTargets, trimmedCreator) : new Set<string>()),
      [actionTargets, trimmedCreator],
    )
    const focusMatchTarget = focusActor ? { name: focusActor, aliases: focusObj?.aliases } : null

    return (
      <ImageViewer
        images={viewerImages}
        value={viewerId}
        onValueChange={setViewerId}
        privateMedia
        label="人像与对照核对"
        labels={VIEWER_LABELS}
      >
        <div className="ws-inner-body ws-actor-inspector">
          {!hideMedia && task && (
            <>
              <h3 className="ws-media-title">{task.name}</h3>
              <WorkspaceMedia path={task.path} name={task.name} poster={poster} />
            </>
          )}
          {!!task?.manual && <WorkspaceMessage>已人工接管，部分分析未完成。</WorkspaceMessage>}

          {/* 1. Top BEUI Combobox Actor Search & Switcher */}
          <div className="ws-actor-topbar">
            <Combobox
              value={trimmedCreator || undefined}
              onValueChange={handleSelectPerson}
              open={comboOpen}
              onOpenChange={setComboOpen}
              query={query}
              onQueryChange={setQuery}
              filter={() => true}
              disabled={false}
              labels={COMBOBOX_LABELS}
              className="min-w-0 flex-1"
            >
              <ComboboxTrigger className="ws-actor-combobox-trigger">
                <ComboboxInput
                  ref={comboInputRef}
                  displayValue={trimmedCreator}
                  placeholder="搜索人物、别名或输入新名…"
                  aria-label="搜索或切换人物"
                  endAdornment={
                    !comboOpen && !query ? (
                      <Kbd className="hidden lg:inline-flex">P</Kbd>
                    ) : undefined
                  }
                />
              </ComboboxTrigger>
              <ComboboxContent align="start" className="ws-actor-combobox-content">
                <ComboboxList ariaLabel="可选人物" className="ws-actor-combobox-list">
                  <ComboboxEmpty>
                    {peopleLoading && !people.length ? '读取人物中…' : '没有匹配的人物'}
                  </ComboboxEmpty>
                  {recItems.length > 0 && (
                    <ComboboxGroup>
                      <ComboboxLabel>推荐与关联</ComboboxLabel>
                      {recItems.map(renderComboboxItem)}
                    </ComboboxGroup>
                  )}
                  {recentItems.length > 0 && (
                    <ComboboxGroup>
                      {recItems.length > 0 && <ComboboxSeparator />}
                      <ComboboxLabel>最近归名</ComboboxLabel>
                      {recentItems.map(renderComboboxItem)}
                    </ComboboxGroup>
                  )}
                  {allItems.length > 0 && (
                    <ComboboxGroup>
                      {(recItems.length > 0 || recentItems.length > 0) && <ComboboxSeparator />}
                      <ComboboxLabel>
                        {trimmedQuery ? `搜索结果 · ${rankedChoices.length}` : `全部人物 · ${people.length}`}
                      </ComboboxLabel>
                      {allItems.map(renderComboboxItem)}
                    </ComboboxGroup>
                  )}
                </ComboboxList>
                {canQuickCreateFromSearch && (
                  <div className="border-t border-border p-1.5">
                    <Button
                      variant="secondary"
                      size="sm"
                      className="w-full justify-center"
                      onPointerDown={e => e.preventDefault()}
                      onClick={() => startCreate(trimmedQuery)}
                    >
                      <UserPlus size={13} aria-hidden="true" />
                      新建「{trimmedQuery}」
                    </Button>
                  </div>
                )}
              </ComboboxContent>
            </Combobox>

            <Button
              variant={creating ? 'secondary' : 'outline'}
              size="icon"
              aria-label="新建人物"
              aria-pressed={creating}
              title="新建人物"
              onClick={() => (creating ? cancelCreate() : startCreate())}
            >
              <UserPlus size={15} aria-hidden="true" />
            </Button>
          </div>

          {peopleError && (
            <WorkspaceMessage error>
              {peopleError}
              {onRetryPeople && (
                <Button variant="outline" disabled={peopleLoading} onClick={onRetryPeople}>
                  重试
                </Button>
              )}
            </WorkspaceMessage>
          )}

          {/* 2. Inline BEUI Collapsible Create Person Card */}
          <Collapsible open={creating}>
            <CollapsibleContent>
              {creating && (
                <Card size="compact" className="ws-create-person">
                  <div className="ws-create-person-head">
                    <div className="ws-create-person-identity">
                      {sampleFaces.length > 0 ? (
                        <AvatarGroup
                          size="sm"
                          max={4}
                          label="所选视频人脸采样"
                          members={sampleFaces.slice(0, 5).map((src, idx) => ({
                            id: `sample-${idx}`,
                            name: trimmedCreator || `人脸 ${idx + 1}`,
                            src,
                          }))}
                        />
                      ) : (
                        <Avatar name={trimmedCreator || '新'} size="sm" />
                      )}
                      <div className="ws-create-person-meta">
                        <strong>{trimmedCreator || '未命名'}</strong>
                      </div>
                    </div>
                    <AnimatedBadge
                      size="sm"
                      showIcon={false}
                      status={
                        validationError
                          ? 'danger'
                          : !trimmedCreator
                            ? 'neutral'
                            : existingExactOrAlias
                              ? 'warning'
                              : 'success'
                      }
                    >
                      {validationError
                        ? '格式有误'
                        : !trimmedCreator
                          ? '待输入'
                          : existingExactOrAlias
                            ? '已存在'
                            : '新人物'}
                    </AnimatedBadge>
                  </div>

                  <WorkspaceField label="人物名称">
                    <WorkspaceInput
                      ref={newInputRef}
                      value={creator}
                      maxLength={120}
                      placeholder="输入账号 ID 或昵称…"
                      aria-label="归名人物"
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="none"
                      onChange={e => onChoose(e.target.value)}
                      onKeyDown={e => {
                        if (e.key === 'Enter' && trimmedCreator && !validationError) {
                          e.preventDefault()
                          commitCreate()
                        } else if (e.key === 'Escape') {
                          e.preventDefault()
                          cancelCreate()
                        }
                      }}
                    />
                  </WorkspaceField>

                  {validationError && <WorkspaceMessage error>{validationError}</WorkspaceMessage>}

                  {validHints.length > 0 && (
                    <div className="ws-create-hints" role="group" aria-label="从所选视频提取的名称建议">
                      <span className="ws-create-hints-label">线索</span>
                      <div className="ws-create-hints-list">
                        {validHints.slice(0, 5).map(hint => (
                          <Button
                            key={hint.name}
                            variant="outline"
                            size="sm"
                            className="ws-create-hint-pill"
                            onClick={() => {
                              onChoose(hint.name)
                              newInputRef.current?.focus()
                            }}
                          >
                            <Sparkles size={11} aria-hidden="true" />
                            <span>{hint.name}</span>
                            <small className="text-muted-foreground">{hint.source}</small>
                          </Button>
                        ))}
                      </div>
                    </div>
                  )}

                  {existingExactOrAlias && (
                    <Card size="compact" variant="muted" className="ws-create-conflict">
                      <div className="ws-create-conflict-row">
                        <Avatar
                          name={existingExactOrAlias.name}
                          src={existingExactOrAlias.rep_thumb || undefined}
                          size="sm"
                        />
                        <div className="ws-create-conflict-copy">
                          <strong>已存在「{existingExactOrAlias.name}」</strong>
                          {normPersonText(existingExactOrAlias.name) !== normalizedDraft &&
                            existingExactOrAlias.aliases?.length && (
                              <small>别名：{existingExactOrAlias.aliases.join(' / ')}</small>
                            )}
                        </div>
                        <Button
                          variant="secondary"
                          size="sm"
                          onClick={() => {
                            onChoose(existingExactOrAlias.name)
                            setCreating(false)
                          }}
                        >
                          <Check size={12} aria-hidden="true" />
                          选用
                        </Button>
                      </div>
                    </Card>
                  )}

                  {similarExisting.length > 0 && (
                    <div className="ws-create-similar">
                      <span className="ws-create-hints-label">相似已有</span>
                      <div className="ws-create-hints-list">
                        {similarExisting.map(person => (
                          <Button
                            key={person.name}
                            variant="outline"
                            size="sm"
                            className="ws-create-hint-pill"
                            onClick={() => {
                              onChoose(person.name)
                              setCreating(false)
                            }}
                          >
                            <Avatar name={person.name} src={person.rep_thumb || undefined} size="sm" />
                            <span>{person.name}</span>
                          </Button>
                        ))}
                      </div>
                    </div>
                  )}

                  <div className="ws-create-actions">
                    <Button variant="ghost" onClick={cancelCreate}>
                      <ArrowLeft size={14} aria-hidden="true" />
                      返回
                    </Button>
                    <Button
                      variant="primary"
                      disabled={!trimmedCreator || Boolean(validationError)}
                      onClick={commitCreate}
                    >
                      <UserPlus size={14} aria-hidden="true" />
                      {existingExactOrAlias
                        ? `选用 ${existingExactOrAlias.name}`
                        : `使用${trimmedCreator ? `「${trimmedCreator}」` : '新人物'}`}
                    </Button>
                  </div>
                </Card>
              )}
            </CollapsibleContent>
          </Collapsible>

          {/* 3. Multi-selection Batch Scope Bar (only when >1 video selected) */}
          {actionTargets.length > 1 && (
            <Card size="compact" variant="muted" className="ws-actor-scope-card">
              <div className="ws-actor-scope-head">
                <div className="flex min-w-0 items-center gap-1.5 text-xs">
                  <span className="font-medium text-foreground">本次归名</span>
                  <span className="ws-count">
                    <DigitSwap value={actionTargets.length} />
                  </span>
                  {groupLabel && <span className="truncate text-muted-foreground">{groupLabel}</span>}
                  {hiddenCount > 0 && (
                    <span className="shrink-0 text-[11px] text-muted-foreground">+{hiddenCount} 不在本页</span>
                  )}
                </div>
                {actionTargets.length > 1 && (
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-expanded={scopeOpen}
                    onClick={() => setScopeOpen(o => !o)}
                  >
                    {scopeOpen ? '收起' : '明细'}
                    {scopeOpen ? <ChevronUp size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
                  </Button>
                )}
              </div>

              <Collapsible open={scopeOpen && actionTargets.length > 1}>
                <CollapsibleContent>
                  <div className="ws-confirm-list mt-2" aria-label="本次完整归名范围">
                    {actionTargets.map(t => {
                      const summary = reviewListSummary(t)
                      const tThumbs = t.data.face?.thumbs?.slice(0, 3) || []
                      const isPreviewing = selectionPreviewVid === t.vid
                      const itemLock = lockedVids?.get(t.vid)
                      return (
                        <div
                          key={t.vid}
                          className="ws-confirm-file"
                          data-active={isPreviewing || undefined}
                          data-locked={Boolean(itemLock) || undefined}
                        >
                          <button
                            type="button"
                            className="ws-confirm-thumb"
                            aria-label={`预览 ${t.name}`}
                            title="预览此视频"
                            onClick={() => onPreviewTarget?.(t)}
                          >
                            <VideoCover
                              video={{
                                path: t.path,
                                thumb: tThumbs[0] ? `/thumb?p=${encodeURIComponent(tThumbs[0])}` : null,
                                size: t.size,
                                mtime_ns: t.mtime_ns,
                              }}
                              sampleTime={summary.sampleTime}
                            />
                            <span className="ws-confirm-thumb-play">
                              <Play size={11} aria-hidden="true" />
                            </span>
                          </button>
                          <div className="ws-confirm-body">
                            <div className="ws-confirm-top">
                              <Button
                                variant="ghost"
                                className="ws-confirm-name"
                                onClick={() => onPreviewTarget?.(t)}
                                aria-label={`核对所选 ${t.name}`}
                              >
                                <Play size={12} aria-hidden="true" />
                                <span>{t.name}</span>
                              </Button>
                              {itemLock && (
                                <AnimatedBadge
                                  size="sm"
                                  status={lockBadgeStatus(itemLock.status)}
                                  className="h-5 px-1.5 text-[10px]"
                                >
                                  {itemLock.label}
                                </AnimatedBadge>
                              )}
                              {actionTargets.length > 1 && onToggleTarget && (
                                <Button
                                  className="ws-icon"
                                  variant="ghost"
                                  size="sm"
                                  aria-label={`取消选择 ${t.name}`}
                                  title="取消选择"
                                  onClick={() => onToggleTarget(t)}
                                >
                                  <X size={14} aria-hidden="true" />
                                </Button>
                              )}
                            </div>
                            <WorkspacePath value={t.path} compact />
                          </div>
                        </div>
                      )
                    })}
                  </div>
                </CollapsibleContent>
              </Collapsible>
            </Card>
          )}

          {/* 4. Unified Actor-First Candidate & Visual Comparison Cards */}
          {!creating && actorCards.length > 0 && (
            <div className="ws-actor-list" role="list" aria-label="候选人物">
              {actorCards.map(actor => {
                const isSelected = Boolean(trimmedCreator && trimmedCreator === actor.name)
                const isExpanded = actor.name === focusActor
                const actorCatalog = catalogMap.get(actor.name)
                const actorSuggested = suggestionMap.get(actor.name)
                const isExisting = existingPeople
                  ? !existingPeople.size || existingPeople.has(actor.name)
                  : !catalogReady || catalogMap.has(actor.name)
                const actorLibThumb = isExisting
                  ? actorCatalog?.rep_thumb || (!actor.is_new ? actor.rep_thumb : null) || null
                  : null
                const actorFaceMatchIdx = faceMatches.findIndex(m => m.name === actor.name)
                const actorFaceMatch = actorFaceMatchIdx >= 0 ? faceMatches[actorFaceMatchIdx] : undefined
                const actorFaceScore = typeof actorFaceMatch?.score === 'number' ? actorFaceMatch.score : actor.sim
                const actorFaceSupport =
                  typeof actorFaceMatch?.support === 'number' ? actorFaceMatch.support : actor.face_support
                const actorFaceTime =
                  typeof actorFaceMatch?.timestamp === 'number' ? actorFaceMatch.timestamp : actor.face_timestamp
                const actorFaceThumbSrc = toThumbSrc(actorFaceMatch?.thumb) || actor.face_thumb || null
                const actorAliases = actorCatalog?.aliases || actor.aliases
                const actorMatchedMarks = marks.filter(m =>
                  watermarkMatchesPerson(m, { name: actor.name, aliases: actorAliases }, marks),
                )
                const hasFace = Boolean(
                  actor.sources?.includes('face') || actorFaceMatch || typeof actorFaceScore === 'number',
                )
                const hasWatermark = Boolean(actor.sources?.includes('watermark') || actorMatchedMarks.length > 0)
                const hasFolder = Boolean(actor.sources?.includes('folder'))
                const isRelated = actorSuggested?.tier === 'related' || actor.tier === 'related'
                const actorLock = lockedPeople?.get(actor.name)
                const destructiveLock =
                  actorLock && isDestructivePersonAction(actorLock.action) ? actorLock : undefined

                const metaParts: string[] = []
                if (typeof actorFaceSupport === 'number') {
                  metaParts.push(`${actorFaceSupport}脸${formatTimestamp(actorFaceTime) ? ` ${formatTimestamp(actorFaceTime)}` : ''}`)
                }
                if (actorMatchedMarks[0]?.text) {
                  metaParts.push(actorMatchedMarks[0].text)
                } else if (actorSuggested?.match_reason && !hasFace && !hasWatermark) {
                  metaParts.push(actorSuggested.match_reason)
                }
                if (isRelated && actorAliases?.length) {
                  metaParts.push(`别名 ${actorAliases.slice(0, 2).join(' / ')}`)
                }

                const personViewerId = `person:${actor.name}`
                const matchViewerId = actorFaceMatchIdx >= 0 ? `match:${actor.name}:${actorFaceMatchIdx}` : null
                const avatarViewerId =
                  isExisting && viewerImageIds.has(personViewerId)
                    ? personViewerId
                    : matchViewerId && viewerImageIds.has(matchViewerId)
                      ? matchViewerId
                      : null

                const compareVideoFaceSrc = actorFaceThumbSrc || sampleFaces[0] || null
                const compareVideoFaceViewerId =
                  compareVideoFaceSrc &&
                  viewerImages.find(
                    img =>
                      img.src === compareVideoFaceSrc &&
                      (img.id.startsWith('match:') || img.id.startsWith('sample:')),
                  )?.id

                return (
                  <Card
                    key={actor.name}
                    role="listitem"
                    size="compact"
                    selected={isSelected}
                    data-expanded={isExpanded || undefined}
                    className="ws-actor-card"
                  >
                    <div className="ws-actor-card-head">
                      {avatarViewerId ? (
                        <ImageViewerThumbnail
                          imageId={avatarViewerId}
                          className="ws-actor-avatar-trigger"
                          imageClassName="ws-vc-avatar-img"
                        />
                      ) : (
                        <Avatar
                          name={actor.name}
                          src={actorLibThumb || undefined}
                          size="md"
                          className="ws-actor-avatar"
                        />
                      )}

                      <button
                        type="button"
                        className="ws-actor-card-select"
                        aria-pressed={isSelected}
                        disabled={busy || Boolean(destructiveLock)}
                        onClick={() => handleSelectPerson(actor.name)}
                      >
                        <span className="ws-actor-card-title">
                          <strong title={actor.name}>{actor.name}</strong>
                          {actorLock ? (
                            <AnimatedBadge
                              size="sm"
                              status={lockBadgeStatus(actorLock.status)}
                              className="h-4 px-1.5 text-[10px]"
                            >
                              {actorLock.label}
                            </AnimatedBadge>
                          ) : null}
                          {hasFace && (
                            <span className="ws-ev-tag" data-kind="face">
                              <ScanFace size={11} aria-hidden="true" />
                              {typeof actorFaceScore === 'number' ? actorFaceScore.toFixed(2) : '人脸'}
                            </span>
                          )}
                          {hasWatermark && (
                            <span className="ws-ev-tag" data-kind="text">
                              <FileText size={11} aria-hidden="true" />
                              水印
                            </span>
                          )}
                          {isRelated && !hasFace && !hasWatermark && (
                            <span className="ws-ev-tag" data-kind="related">
                              <Link2 size={11} aria-hidden="true" />
                              关联
                            </span>
                          )}
                          {hasFolder && (
                            <span className="ws-ev-tag" data-kind="folder">
                              <Folder size={11} aria-hidden="true" />
                              目录
                            </span>
                          )}
                          {!isExisting && (
                            <span className="ws-ev-tag" data-kind="new">
                              新名
                            </span>
                          )}
                        </span>
                        {metaParts.length > 0 && (
                          <span className="ws-actor-card-meta tabular-nums">{metaParts.join(' · ')}</span>
                        )}
                      </button>

                      <div className="ws-actor-card-actions">
                        {isSelected && (
                          <Check size={14} className="ws-actor-selected-mark" aria-hidden="true" />
                        )}
                        {isExisting ? (
                          <Button
                            variant={activeReferenceName === actor.name && stageTab === 'reference' ? 'secondary' : 'ghost'}
                            size="sm"
                            className="ws-actor-ref-trigger"
                            disabled={busy}
                            aria-label={`看 ${actor.name} 的对照视频`}
                            title="在舞台播放对照视频"
                            onClick={() => onReference(actor.name, 0)}
                          >
                            <ActionSwapIcon
                              value={activeReferenceName === actor.name && stageTab === 'reference' ? 'open' : 'closed'}
                              animation="roll"
                            >
                              <Eye size={13} />
                            </ActionSwapIcon>
                            {isExpanded && refData && (
                              <span className="ws-count">
                                <DigitSwap value={refData.total} />
                              </span>
                            )}
                          </Button>
                        ) : (
                          isExpanded && (
                            <Button
                              variant="outline"
                              size="sm"
                              disabled={busy}
                              aria-label={`修改新人物名称 ${actor.name}`}
                              onClick={() => startCreate(actor.name)}
                            >
                              <Pencil size={12} aria-hidden="true" />
                              改名
                            </Button>
                          )
                        )}
                      </div>
                    </div>

                    <Collapsible open={isExpanded}>
                      <CollapsibleContent>
                        {isExpanded && (
                          <div className="ws-actor-card-detail">
                            {compareVideoFaceSrc && (
                              <div className="ws-vc-face-compare" aria-label="视频人脸与库内头像对比">
                                <div className="ws-vc-face-slot">
                                  {compareVideoFaceViewerId ? (
                                    <ImageViewerThumbnail
                                      imageId={compareVideoFaceViewerId}
                                      className="ws-vc-face-thumb"
                                      imageClassName="ws-vc-face-img"
                                    />
                                  ) : (
                                    <PrivateImage
                                      src={compareVideoFaceSrc}
                                      alt="视频人脸"
                                      compact
                                      className="ws-vc-face-static"
                                    />
                                  )}
                                  <span>视频人脸</span>
                                </div>
                                <div className="ws-vc-face-bridge">
                                  <ArrowLeftRight size={13} className="text-muted-foreground" aria-hidden="true" />
                                  {typeof actorFaceScore === 'number' ? (
                                    <>
                                      <AnimatedBadge
                                        size="sm"
                                        status="success"
                                        showIcon={false}
                                        className="h-5 px-1.5 text-[10px]"
                                      >
                                        {actorFaceScore.toFixed(3)}
                                      </AnimatedBadge>
                                      {(typeof actorFaceSupport === 'number' || formatTimestamp(actorFaceTime)) && (
                                        <small className="tabular-nums">
                                          {[
                                            typeof actorFaceSupport === 'number' ? `${actorFaceSupport}脸` : '',
                                            formatTimestamp(actorFaceTime),
                                          ]
                                            .filter(Boolean)
                                            .join(' · ')}
                                        </small>
                                      )}
                                    </>
                                  ) : (
                                    <small>人像对照</small>
                                  )}
                                </div>
                                <div className="ws-vc-face-slot">
                                  {isExisting && viewerImageIds.has(personViewerId) ? (
                                    <ImageViewerThumbnail
                                      imageId={personViewerId}
                                      className="ws-vc-face-thumb"
                                      imageClassName="ws-vc-face-img"
                                    />
                                  ) : (
                                    <Avatar
                                      name={actor.name}
                                      src={actorLibThumb || undefined}
                                      size="lg"
                                      className="ws-vc-face-fallback"
                                    />
                                  )}
                                  <span>
                                    {!isExisting ? '新人物（待建）' : actorLibThumb ? '库内头像' : '暂无库内头像'}
                                  </span>
                                </div>
                              </div>
                            )}

                            {isExisting ? (
                              refData ? (
                                refData.error ? (
                                  <WorkspaceMessage error>
                                    {refData.error}
                                    {retryRef && (
                                      <Button variant="outline" onClick={retryRef}>
                                        重试
                                      </Button>
                                    )}
                                  </WorkspaceMessage>
                                ) : refData.videos.length > 0 ? (
                                  <div className="ws-vc-ref-grid" role="group" aria-label={`${actor.name} 的已归档对照视频`}>
                                    {refData.videos.slice(0, 3).map((video, idx) => (
                                      <div key={video.vid} className="ws-vc-ref-tile">
                                        <Button
                                          variant="ghost"
                                          className="ws-vc-ref-btn"
                                          aria-label={`在舞台播放对照 ${video.name}`}
                                          title={video.name}
                                          onClick={() => onReference(actor.name, idx)}
                                        >
                                          <span className="ws-vc-ref-cover">
                                            <VideoCover video={{ path: video.path || video.vid, thumb: video.thumb }} />
                                          </span>
                                          <span className="ws-vc-ref-name">{video.name}</span>
                                        </Button>
                                        {video.thumb && viewerImageIds.has(`ref:${video.vid}`) && (
                                          <Button
                                            variant="ghost"
                                            size="icon"
                                            className="ws-vc-ref-zoom"
                                            aria-label={`放大查看 ${video.name} 缩略图`}
                                            title="放大查看缩略图"
                                            onClick={() => setViewerId(`ref:${video.vid}`)}
                                          >
                                            <Maximize2 size={11} aria-hidden="true" />
                                          </Button>
                                        )}
                                      </div>
                                    ))}
                                  </div>
                                ) : (
                                  <WorkspaceMessage>暂无已归档对照视频。</WorkspaceMessage>
                                )
                              ) : refLoading ? (
                                <WorkspaceMessage>读取对照视频…</WorkspaceMessage>
                              ) : null
                            ) : (
                              <WorkspaceMessage>
                                库内尚无「{actor.name}」，可直接新建或从上方切换。
                              </WorkspaceMessage>
                            )}

                            {actorMatchedMarks.length > 0 && task && (
                              <div className="ws-actor-wm-list" aria-label={`${actor.name} 命中的水印证据`}>
                                {actorMatchedMarks.slice(0, 2).map(mark => (
                                  <div key={mark.key} className="ws-actor-wm-item">
                                    <div className="ws-actor-wm-head">
                                      <div className="flex min-w-0 items-center gap-1.5">
                                        <span className="truncate font-medium text-foreground">{mark.text}</span>
                                        {confirmedWatermarkKeys(actionTargets, actor.name).has(mark.key) && (
                                          <AnimatedBadge
                                            size="sm"
                                            status="success"
                                            showIcon={false}
                                            className="h-4 px-1 text-[10px]"
                                          >
                                            已记住
                                          </AnimatedBadge>
                                        )}
                                      </div>
                                      <span className="ws-muted shrink-0 tabular-nums">{mark.frames ?? 0} 帧</span>
                                    </div>
                                    <PrivateImage
                                      compact
                                      src={`/api/watermarks/evidence?vid=${encodeURIComponent(task.vid)}&key=${encodeURIComponent(mark.key)}`}
                                      alt={`水印：${mark.text}`}
                                      loading="lazy"
                                      className="ws-actor-wm-img"
                                    />
                                  </div>
                                ))}
                              </div>
                            )}

                            {isExisting && <ExternalProfiles name={actor.name} compact />}
                          </div>
                        )}
                      </CollapsibleContent>
                    </Collapsible>
                  </Card>
                )
              })}
            </div>
          )}

          {!creating && actorCards.length === 0 && (
            <>
              <WorkspaceMessage>暂无候选人物，可在上方搜索或新建。</WorkspaceMessage>
              {faceThumbs.length > 0 && (
                <div className="ws-face-samples" role="group" aria-label="当前视频人脸采样">
                  {faceThumbs.slice(0, 6).map((thumbPath, idx) =>
                    viewerImageIds.has(`sample:${idx}`) ? (
                      <ImageViewerThumbnail
                        key={thumbPath}
                        imageId={`sample:${idx}`}
                        className="ws-face-sample-thumb"
                        imageClassName="ws-face-sample-img"
                      />
                    ) : (
                      <PrivateImage
                        compact
                        key={thumbPath}
                        src={`/thumb?p=${encodeURIComponent(thumbPath)}`}
                        alt="人脸采样证据"
                        loading="lazy"
                        className="ws-face-sample"
                      />
                    ),
                  )}
                </div>
              )}
            </>
          )}

          {/* 5. Default-Collapsed Secondary Drawer: Raw Evidence & Note */}
          {task && !creating && (
            <WorkspaceSection
              title={note !== task.note ? '更多线索与备注 · 未保存' : '更多线索与备注'}
              count={marks.length + faceThumbs.length + (note ? 1 : 0)}
              defaultOpen={false}
            >
              {faceThumbs.length > 0 && (
                <div className="ws-face-samples" role="group" aria-label="当前视频人脸采样">
                  {faceThumbs.slice(0, 6).map((thumbPath, idx) =>
                    viewerImageIds.has(`sample:${idx}`) ? (
                      <ImageViewerThumbnail
                        key={thumbPath}
                        imageId={`sample:${idx}`}
                        className="ws-face-sample-thumb"
                        imageClassName="ws-face-sample-img"
                      />
                    ) : (
                      <PrivateImage
                        compact
                        key={thumbPath}
                        src={`/thumb?p=${encodeURIComponent(thumbPath)}`}
                        alt="人脸采样证据"
                        loading="lazy"
                        className="ws-face-sample"
                      />
                    ),
                  )}
                </div>
              )}

              {marks.length > 0 &&
                marks.map(mark => {
                  const matchedCurrent = watermarkMatchesPerson(mark, focusMatchTarget, marks)
                  const alreadyConfirmed = confirmedForCreator.has(mark.key)
                  const canRemember =
                    Boolean(trimmedCreator) &&
                    commonMarkKeys.has(mark.key) &&
                    !alreadyConfirmed
                  const armedRule = watermarkKey === mark.key
                  return (
                    <Card
                      key={mark.key}
                      size="compact"
                      selected={matchedCurrent || armedRule}
                      className="ws-wm-card"
                    >
                      <div className="ws-card-heading">
                        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                          <strong>{mark.text}</strong>
                          {alreadyConfirmed ? (
                            <AnimatedBadge size="sm" status="success" showIcon={false} className="h-5 px-1.5 text-[10px]">
                              已记住
                            </AnimatedBadge>
                          ) : matchedCurrent ? (
                            <AnimatedBadge size="sm" status="info" showIcon={false} className="h-5 px-1.5 text-[10px]">
                              命中当前
                            </AnimatedBadge>
                          ) : null}
                        </div>
                        <span className="ws-muted tabular-nums">
                          {mark.frames ?? 0} 帧 · {typeof mark.timestamp === 'number' ? mark.timestamp.toFixed(1) : '0.0'} 秒
                        </span>
                      </div>
                      <PrivateImage
                        src={`/api/watermarks/evidence?vid=${encodeURIComponent(task.vid)}&key=${encodeURIComponent(mark.key)}`}
                        alt={`水印：${mark.text}`}
                        loading="lazy"
                        className="ws-evidence-image"
                      />
                      {mark.platform === 'Telegram' && <p className="ws-muted">频道来源线索</p>}
                      {mark.kind === 'account' && (
                        <ExternalProfiles token={mark.token} platform={mark.platform} compact />
                      )}
                      {sourceConfirm === mark.key ? (
                        <div className="ws-source-confirm">
                          <p>仅保留来源信息，重新分析相关建议，并清空当前勾选。</p>
                          <div className="ws-card-actions">
                            <Button
                              disabled={busy}
                              onClick={async () => {
                                if (await onSource(mark.key)) setSourceConfirm(null)
                              }}
                            >
                              确认仅作来源
                            </Button>
                            <Button variant="ghost" disabled={busy} onClick={() => setSourceConfirm(null)}>
                              取消
                            </Button>
                          </div>
                        </div>
                      ) : (
                        <div className="ws-card-actions">
                          {canRemember && (
                            <Button
                              variant={armedRule ? 'secondary' : 'outline'}
                              size="sm"
                              aria-pressed={armedRule}
                              disabled={busy}
                              title={`归给 ${trimmedCreator} 时一并记住此水印，后续同水印自动识别`}
                              onClick={() => onWatermarkKeyChange(armedRule ? '' : mark.key)}
                            >
                              {armedRule ? (
                                <Check size={13} aria-hidden="true" />
                              ) : (
                                <BookmarkPlus size={13} aria-hidden="true" />
                              )}
                              <span>{armedRule ? '已选记住' : '归名时记住'}</span>
                            </Button>
                          )}
                          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setSourceConfirm(mark.key)}>
                            仅作来源
                          </Button>
                        </div>
                      )}
                    </Card>
                  )
                })}

              {!hideMedia && <WorkspacePath value={task.path} />}
              <WorkspaceField label="备注 / 其他出镜人物">
                <WorkspaceTextarea
                  aria-label="视频备注"
                  value={note}
                  rows={3}
                  maxLength={1000}
                  disabled={busy}
                  onChange={e => onNoteChange(e.target.value)}
                />
              </WorkspaceField>
              <div className="ws-card-actions">
                <span className="ws-muted">{note !== task.note ? '未保存' : ''}</span>
                <Button variant="outline" disabled={busy || note === task.note || locked} onClick={onSaveNote}>
                  <Save size={13} aria-hidden="true" />
                  保存备注
                </Button>
              </div>
            </WorkspaceSection>
          )}
        </div>

        <InspectorViewerContent
          suggestionMap={suggestionMap}
          catalogMap={catalogMap}
          currentValue={trimmedCreator}
          disabled={busy}
          onSelectPerson={handleSelectPerson}
          onComparePerson={(name, refIndex) => onReference(name, refIndex)}
          refVideos={refData?.videos || []}
          refOwner={isFocusExisting ? focusActor : null}
        />
      </ImageViewer>
    )
  },
)

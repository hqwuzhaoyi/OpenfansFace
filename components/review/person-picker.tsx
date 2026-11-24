'use client'
import { useEffect, useRef, useState } from 'react'
import {
  Check,
  Eye,
  EyeOff,
  Play,
  Plus,
  Sparkles,
  X,
  ArrowLeft,
  Maximize2,
  Pencil,
  UserPlus,
} from 'lucide-react'
import {
  findSimilarExistingPeople,
  normPersonText,
  rankPersonChoices,
  type PersonChoice,
  type PersonNameHint,
} from '@/lib/person-picker-model'
import { api } from '@/lib/face-api'
import type { ReviewVideo } from '@/lib/review-ui'
import { VideoCover } from './video-cover'
import { ExternalProfiles } from '@/components/persons/external-profiles'
import { WorkspaceSearch } from '@/components/workbench/controls'
import { Card } from '@/components/beui/motion/card'
import { Collapsible, CollapsibleContent } from '@/components/beui/motion/collapsible'
import { AnimatedBadge } from '@/components/beui/motion/animated-badge'
import { Avatar, AvatarGroup } from '@/components/beui/motion/avatar-group'
import { ActionSwapIcon, ActionSwapText } from '@/components/beui/motion/action-swap'
import { DigitSwap } from '@/components/beui/motion/digit-swap'
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
  WorkspaceInput,
  WorkspaceField,
  WorkspaceMessage,
  WorkspaceMedia,
  WorkspacePath,
} from '@/components/workbench/inner'

export type { PersonChoice, PersonNameHint }

const PICKER_VIEWER_LABELS = {
  label: '人物头像与对照核对',
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

function validateNewPersonName(raw: string): string | null {
  const trimmed = raw.trim()
  if (!trimmed) return null
  if (trimmed.length > 120) return '名称不超过 120 个字符'
  if (/[\\/\x00-\x1f]/.test(raw) || trimmed === '.' || trimmed === '..') {
    return '不能包含路径分隔符 / 或 \\'
  }
  return null
}

function PickerViewerContent({
  suggestionMap,
  catalogMap,
  currentValue,
  disabled,
  onSelectPerson,
  onComparePerson,
  onPlayRef,
  refVideos,
}: {
  suggestionMap: Map<string, PersonChoice>
  catalogMap: Map<string, PersonChoice>
  currentValue: string
  disabled?: boolean
  onSelectPerson: (name: string) => void
  onComparePerson: (name: string) => void
  onPlayRef: (video: ReviewVideo) => void
  refVideos: ReviewVideo[]
}) {
  const { image, close } = useImageViewer()
  const id = image?.id || ''
  const isSample = id.startsWith('sample:')
  const isPerson = id.startsWith('person:')
  const isRef = id.startsWith('ref:')
  const personName = isPerson ? id.slice('person:'.length) : null
  const refVid = isRef ? id.slice('ref:'.length) : null
  const refVideo = refVid ? refVideos.find(v => v.vid === refVid) : undefined
  const suggested = personName ? suggestionMap.get(personName) : undefined
  const personObj = personName ? catalogMap.get(personName) || suggested : undefined
  const canCompare = Boolean(personName && (catalogMap.size === 0 || catalogMap.has(personName)))
  const kindBadge = isSample
    ? '人脸采样'
    : isRef
      ? '对照'
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
                  onComparePerson(personName)
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
        {refVideo && (
          <button
            type="button"
            onClick={() => {
              onPlayRef(refVideo)
              close()
            }}
            className="inline-flex h-8 shrink-0 items-center gap-1 rounded-full bg-white px-3 text-xs font-medium text-neutral-950 transition-colors hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white"
          >
            <Play size={12} aria-hidden="true" />
            播放
          </button>
        )}
      </div>
      <ImageViewerNext />
    </ImageViewerContent>
  )
}

export function PersonPicker({
  people,
  suggestions = [],
  sampleFaces = [],
  nameHints = [],
  value,
  onChange,
  disabled,
  loading = false,
  error = '',
  onRetry,
  onReference,
  search,
  onSearchChange,
}: {
  search?: string
  onSearchChange?: (value: string) => void
  compact?: boolean
  people: PersonChoice[]
  suggestions?: PersonChoice[]
  sampleFaces?: string[]
  nameHints?: PersonNameHint[]
  value: string
  onChange: (value: string) => void
  disabled?: boolean
  loading?: boolean
  error?: string
  onRetry?: () => void
  onReference?: (name: string) => void
}) {
  const [localQuery, setLocalQuery] = useState('')
  const [creating, setCreating] = useState(false)
  const [comparing, setComparing] = useState<string | null>(null)
  const [viewerId, setViewerId] = useState<string | null>(null)
  const [refState, setRefState] = useState<{
    name: string
    videos: ReviewVideo[]
    total: number
    error?: string
  } | null>(null)
  const [refPlaying, setRefPlaying] = useState<ReviewVideo | null>(null)
  const [refRetry, setRefRetry] = useState(0)

  const previous = useRef(value)
  const newInput = useRef<HTMLInputElement>(null)
  const query = search ?? localQuery
  const setQuery = onSearchChange ?? setLocalQuery

  const suggestionMap = new Map(suggestions.map(s => [s.name, s]))
  const catalogMap = new Map(people.map(p => [p.name, p]))
  const catalogReady = people.length > 0

  // Merge suggestions first so candidates and related people stay pinned at the top
  const combined = [
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

  const trimmedQuery = query.trim()
  const choices = rankPersonChoices(combined, query)
  const trimmedValue = value.trim()
  const isExistingPerson = !!trimmedValue && (!catalogReady || catalogMap.has(trimmedValue))
  const isNewPerson = !!trimmedValue && catalogReady && !catalogMap.has(trimmedValue)
  const selectedPerson = trimmedValue
    ? combined.find(p => p.name === trimmedValue) || {
        name: trimmedValue,
        rep_thumb: sampleFaces[0] || null,
      }
    : null

  // Match check while creating a new person: detect exact or alias match in existing catalog
  const normalizedDraft = normPersonText(value)
  const validationError = creating ? validateNewPersonName(value) : null
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
      ? findSimilarExistingPeople(people, trimmedValue, 3)
      : []

  // Deduplicate name hints and filter out invalid names
  const validHints = nameHints.filter(
    h => h.name.trim() && !validateNewPersonName(h.name) && h.name.trim() !== trimmedValue,
  )

  useEffect(() => {
    if (!comparing) {
      setRefPlaying(null)
      return
    }
    const abort = new AbortController()
    setRefPlaying(null)
    setRefState(prev => (prev?.name === comparing ? prev : null))
    api<{ vids: ReviewVideo[]; total: number }>(
      `/api/persons/reference?name=${encodeURIComponent(comparing)}`,
      { signal: abort.signal },
    )
      .then(data => {
        if (!abort.signal.aborted) {
          setRefState({ name: comparing, videos: data.vids, total: data.total })
        }
      })
      .catch(err => {
        if (!abort.signal.aborted) {
          setRefState({ name: comparing, videos: [], total: 0, error: err.message })
        }
      })
    return () => abort.abort()
  }, [comparing, refRetry])

  const startCreate = (initialName?: string) => {
    previous.current = value
    const seed =
      typeof initialName === 'string'
        ? initialName
        : trimmedQuery || (isNewPerson ? trimmedValue : validHints[0]?.name || '')
    setCreating(true)
    onChange(seed)
    requestAnimationFrame(() => {
      newInput.current?.focus()
      newInput.current?.select()
    })
  }

  const cancelCreate = () => {
    onChange(previous.current)
    setCreating(false)
  }

  const commitCreate = () => {
    if (!trimmedValue || validationError) return
    if (existingExactOrAlias) {
      onChange(existingExactOrAlias.name)
    } else {
      onChange(trimmedValue)
    }
    setCreating(false)
  }

  const toggleCompare = (name: string) => {
    setComparing(current => (current === name ? null : name))
  }

  const currentRef = refState?.name === comparing ? refState : null
  const canQuickCreateFromSearch =
    !creating &&
    !!trimmedQuery &&
    !validateNewPersonName(trimmedQuery) &&
    !people.some(p => normPersonText(p.name) === normPersonText(trimmedQuery))

  // Build deduplicated Lightbox images: current video sample faces -> visible person avatars -> inline reference thumbs
  const viewerImageMap = new Map<string, LightboxImage>()
  const visibleSamples = !creating ? sampleFaces.slice(0, 4) : []
  visibleSamples.forEach((src, idx) => {
    if (!src) return
    const id = `sample:${idx}`
    viewerImageMap.set(id, {
      id,
      src,
      alt: `人脸采样 ${idx + 1}`,
      width: 400,
      height: 400,
      caption: `当前视频人脸采样 ${idx + 1}`,
    })
  })
  if (!creating) {
    for (const person of choices) {
      if (!person.rep_thumb) continue
      const id = `person:${person.name}`
      if (!viewerImageMap.has(id)) {
        viewerImageMap.set(id, {
          id,
          src: person.rep_thumb,
          alt: person.name,
          width: 400,
          height: 400,
          caption: person.name,
        })
      }
    }
    if (selectedPerson && !isNewPerson && selectedPerson.rep_thumb) {
      const id = `person:${selectedPerson.name}`
      if (!viewerImageMap.has(id)) {
        viewerImageMap.set(id, {
          id,
          src: selectedPerson.rep_thumb,
          alt: selectedPerson.name,
          width: 400,
          height: 400,
          caption: selectedPerson.name,
        })
      }
    }
    for (const video of (currentRef?.videos || []).slice(0, 6)) {
      if (!video.thumb) continue
      const id = `ref:${video.vid}`
      if (!viewerImageMap.has(id)) {
        viewerImageMap.set(id, {
          id,
          src: video.thumb,
          alt: video.name,
          width: 640,
          height: 400,
          caption: `${comparing} · ${video.name}`,
        })
      }
    }
  }
  const viewerImages = [...viewerImageMap.values()]
  const viewerImageIds = new Set(viewerImages.map(img => img.id))

  const selectedSuggested = selectedPerson ? suggestionMap.get(selectedPerson.name) : undefined
  const selectedMeta = selectedPerson
    ? selectedSuggested?.match_reason ||
      (selectedPerson.aliases?.length
        ? `别名：${selectedPerson.aliases.slice(0, 2).join(' / ')}`
        : '')
    : ''
  const selectedBannerViewerId = selectedPerson
    ? !isNewPerson && viewerImageIds.has(`person:${selectedPerson.name}`)
      ? `person:${selectedPerson.name}`
      : isNewPerson && sampleFaces[0] && viewerImageIds.has('sample:0')
        ? 'sample:0'
        : null
    : null

  const recommendedChoices = !trimmedQuery ? choices.filter(p => suggestionMap.has(p.name)) : []
  const otherChoices = !trimmedQuery ? choices.filter(p => !suggestionMap.has(p.name)) : choices
  const showSections = !trimmedQuery && recommendedChoices.length > 0 && otherChoices.length > 0

  const renderPersonOption = (person: PersonChoice) => {
    const suggested = suggestionMap.get(person.name)
    const isComparing = comparing === person.name
    const isSelected = value === person.name
    const canCompare = !catalogReady || catalogMap.has(person.name)
    const thumbId = `person:${person.name}`
    const hasThumbViewer = viewerImageIds.has(thumbId)
    const subtitle =
      suggested?.match_reason ||
      (person.aliases?.length ? person.aliases.slice(0, 2).join(' · ') : '')

    return (
      <div
        key={person.name}
        className="ws-person-option"
        data-selected={isSelected || undefined}
        data-suggested={
          suggested ? (suggested.tier === 'related' ? 'related' : 'true') : undefined
        }
      >
        {hasThumbViewer ? (
          <ImageViewerThumbnail
            imageId={thumbId}
            disabled={disabled}
            className="ws-person-thumb-trigger"
            imageClassName="ws-person-thumb-img"
          />
        ) : (
          <Avatar
            name={person.name}
            src={person.rep_thumb || undefined}
            size="lg"
            className="ws-person-avatar"
          />
        )}
        <Button
          variant="ghost"
          className="ws-person-select"
          disabled={disabled}
          onClick={() => onChange(person.name)}
          aria-label={`选择人物 ${person.name}`}
          aria-pressed={isSelected}
        >
          <span className="ws-person-name">
            <span className="ws-person-name-row">
              <span>{person.name}</span>
              {suggested && (
                <AnimatedBadge
                  status={
                    suggested.tier === 'related'
                      ? 'info'
                      : suggested.is_new
                        ? 'warning'
                        : 'success'
                  }
                  size="sm"
                  showIcon={suggested.tier !== 'related' && !suggested.is_new}
                  icon={
                    suggested.tier !== 'related' && !suggested.is_new ? (
                      <Sparkles className="size-3" />
                    ) : undefined
                  }
                >
                  {suggested.tier === 'related' ? '关联' : suggested.is_new ? '新名' : '候选'}
                </AnimatedBadge>
              )}
            </span>
            {subtitle && <small>{subtitle}</small>}
          </span>
          {isSelected && <Check size={14} aria-hidden="true" />}
        </Button>
        {canCompare && (
          <Button
            variant={isComparing ? 'secondary' : 'ghost'}
            size="icon"
            disabled={disabled}
            aria-label={`看 ${person.name} 的对照视频`}
            aria-pressed={isComparing}
            title={isComparing ? '收起对照视频' : '展开对照视频'}
            onClick={() => toggleCompare(person.name)}
          >
            <ActionSwapIcon value={isComparing ? 'open' : 'closed'} animation="blur">
              {isComparing ? <EyeOff size={14} /> : <Eye size={14} />}
            </ActionSwapIcon>
          </Button>
        )}
      </div>
    )
  }

  return (
    <ImageViewer
      images={viewerImages}
      value={viewerId}
      onValueChange={setViewerId}
      privateMedia
      label="人物头像与对照核对"
      labels={PICKER_VIEWER_LABELS}
    >
      <section className="ws-person-picker" aria-label="归名人物选择">
        <div className="ws-person-choice-head">
          <div className="ws-person-choice-title">
            <strong>{creating ? '新建人物' : '选择人物'}</strong>
            {!creating && <span className="ws-count tabular-nums">{choices.length}</span>}
          </div>
          {!creating && (
            <Button variant="ghost" size="sm" disabled={disabled} onClick={() => startCreate()}>
              <Plus size={14} aria-hidden="true" />
              新建人物
            </Button>
          )}
        </div>

        {selectedPerson && !creating && (
          <Card size="compact" selected className="ws-person-selected-banner">
            {selectedBannerViewerId ? (
              <button
                type="button"
                className="ws-person-banner-thumb"
                aria-label={`放大查看 ${selectedPerson.name} 头像`}
                title="放大查看头像"
                onClick={() => setViewerId(selectedBannerViewerId)}
              >
                <Avatar
                  name={selectedPerson.name}
                  src={selectedPerson.rep_thumb || sampleFaces[0] || undefined}
                  size="md"
                  className="ws-person-selected-avatar"
                />
              </button>
            ) : (
              <Avatar
                name={selectedPerson.name}
                src={selectedPerson.rep_thumb || sampleFaces[0] || undefined}
                size="md"
                className="ws-person-selected-avatar"
              />
            )}
            <div className="ws-person-selected-copy">
              <div className="ws-person-selected-title">
                <strong title={selectedPerson.name}>{selectedPerson.name}</strong>
                <AnimatedBadge
                  size="sm"
                  status={
                    isNewPerson
                      ? 'warning'
                      : selectedSuggested?.tier === 'related'
                        ? 'info'
                        : selectedSuggested
                          ? 'success'
                          : 'neutral'
                  }
                  showIcon={false}
                >
                  {isNewPerson
                    ? '新名'
                    : selectedSuggested?.tier === 'related'
                      ? '关联'
                      : selectedSuggested
                        ? '候选'
                        : '已选'}
                </AnimatedBadge>
              </div>
              {selectedMeta && <small>{selectedMeta}</small>}
            </div>
            {isNewPerson ? (
              <Button
                variant="outline"
                size="sm"
                disabled={disabled}
                aria-label={`修改新人物名称 ${selectedPerson.name}`}
                title="修改新人物名称"
                onClick={() => startCreate(selectedPerson.name)}
              >
                <Pencil size={12} aria-hidden="true" />
                改名
              </Button>
            ) : (
              <Button
                variant={comparing === selectedPerson.name ? 'secondary' : 'outline'}
                size="sm"
                disabled={disabled}
                aria-label={`看 ${selectedPerson.name} 的对照视频`}
                aria-pressed={comparing === selectedPerson.name}
                title="原位展开对照视频核对"
                onClick={() => toggleCompare(selectedPerson.name)}
              >
                <ActionSwapIcon
                  value={comparing === selectedPerson.name ? 'open' : 'closed'}
                  animation="roll"
                >
                  {comparing === selectedPerson.name ? <EyeOff size={13} /> : <Eye size={13} />}
                </ActionSwapIcon>
                <ActionSwapText
                  value={comparing === selectedPerson.name ? '收起' : '对照'}
                  animation="roll"
                >
                  {comparing === selectedPerson.name ? '收起' : '对照'}
                </ActionSwapText>
              </Button>
            )}
            <Button
              variant="ghost"
              size="icon"
              aria-label="清空归名人物"
              title="清空人物"
              disabled={disabled}
              onClick={() => onChange('')}
            >
              <X size={14} aria-hidden="true" />
            </Button>
          </Card>
        )}

        {/* BEUI Collapsible Inline Reference Drawer: compare person's archived videos without leaving selection mode */}
        <Collapsible open={Boolean(comparing && !creating)}>
          <CollapsibleContent>
            {comparing && !creating && (
              <Card
                size="compact"
                variant="muted"
                className="ws-inline-reference"
                role="region"
                aria-label={`${comparing} 的对照视频`}
              >
                <div className="ws-inline-reference-head">
                  <div className="ws-inline-reference-title">
                    <strong>{comparing}</strong>
                    {currentRef && (
                      <span className="ws-count">
                        <DigitSwap value={currentRef.total} />
                      </span>
                    )}
                  </div>
                  <div className="ws-inline-reference-actions">
                    {value !== comparing && (
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={disabled}
                        onClick={() => onChange(comparing)}
                      >
                        <Check size={12} aria-hidden="true" />
                        选此人
                      </Button>
                    )}
                    {onReference && (
                      <Button
                        variant="ghost"
                        size="icon"
                        disabled={disabled}
                        aria-label={`全屏查看 ${comparing} 的对照视频`}
                        title="打开完整对照面板"
                        onClick={() => onReference(comparing)}
                      >
                        <Maximize2 size={13} aria-hidden="true" />
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label="收起对照视频"
                      title="收起对照"
                      onClick={() => setComparing(null)}
                    >
                      <X size={14} aria-hidden="true" />
                    </Button>
                  </div>
                </div>

                {refPlaying && (
                  <div className="ws-inline-reference-player">
                    <WorkspaceMedia
                      key={refPlaying.vid}
                      path={refPlaying.path || refPlaying.vid}
                      name={refPlaying.name}
                      autoPlay
                    />
                    <WorkspacePath value={refPlaying.path || refPlaying.vid} compact />
                  </div>
                )}

                {!currentRef ? (
                  <WorkspaceMessage>读取对照视频…</WorkspaceMessage>
                ) : currentRef.error ? (
                  <WorkspaceMessage error>
                    {currentRef.error}
                    <Button variant="outline" onClick={() => setRefRetry(n => n + 1)}>
                      重新读取
                    </Button>
                  </WorkspaceMessage>
                ) : currentRef.videos.length ? (
                  <div className="ws-inline-reference-grid">
                    {currentRef.videos.slice(0, 6).map(video => (
                      <div
                        key={video.vid}
                        className="ws-inline-reference-card-wrap"
                        data-pressed={refPlaying?.vid === video.vid || undefined}
                      >
                        <button
                          type="button"
                          className="ws-inline-reference-card"
                          aria-label={`播放对照 ${video.name}`}
                          aria-pressed={refPlaying?.vid === video.vid}
                          onClick={() =>
                            setRefPlaying(prev => (prev?.vid === video.vid ? null : video))
                          }
                        >
                          <div className="ws-inline-reference-thumb">
                            {video.thumb ? (
                              <img
                                src={video.thumb}
                                data-private-media
                                draggable={false}
                                alt=""
                                loading="lazy"
                              />
                            ) : (
                              <VideoCover
                                video={{
                                  path: video.path || video.vid,
                                  thumb: null,
                                }}
                              />
                            )}
                            <span className="ws-inline-reference-play">
                              <Play size={11} aria-hidden="true" />
                            </span>
                          </div>
                          <span className="ws-inline-reference-name" title={video.name}>
                            {video.name}
                          </span>
                        </button>
                        {video.thumb && viewerImageIds.has(`ref:${video.vid}`) && (
                          <button
                            type="button"
                            className="ws-inline-reference-zoom"
                            aria-label={`放大查看 ${video.name} 缩略图`}
                            title="放大查看缩略图"
                            onClick={() => setViewerId(`ref:${video.vid}`)}
                          >
                            <Maximize2 size={11} aria-hidden="true" />
                          </button>
                        )}
                      </div>
                    ))}
                  </div>
                ) : (
                  <WorkspaceMessage>暂无已扫描的对照视频。</WorkspaceMessage>
                )}
                <ExternalProfiles name={comparing} compact />
              </Card>
            )}
          </CollapsibleContent>
        </Collapsible>

        {creating ? (
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
                      name: trimmedValue || `人脸 ${idx + 1}`,
                      src,
                    }))}
                  />
                ) : (
                  <Avatar name={trimmedValue || '新'} size="sm" />
                )}
                <div className="ws-create-person-meta">
                  <strong>{trimmedValue || '未命名'}</strong>
                </div>
              </div>
              <AnimatedBadge
                size="sm"
                status={
                  validationError
                    ? 'danger'
                    : !trimmedValue
                      ? 'neutral'
                      : existingExactOrAlias
                        ? 'warning'
                        : 'success'
                }
              >
                {validationError
                  ? '格式有误'
                  : !trimmedValue
                    ? '待输入'
                    : existingExactOrAlias
                      ? '已存在'
                      : '新人物'}
              </AnimatedBadge>
            </div>

            <WorkspaceField label="人物名称">
              <WorkspaceInput
                ref={newInput}
                value={value}
                disabled={disabled}
                maxLength={120}
                placeholder="输入账号 ID 或昵称…"
                aria-label="归名人物"
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="none"
                onChange={e => onChange(e.target.value)}
                onKeyDown={e => {
                  if (e.key === 'Enter' && trimmedValue && !validationError) {
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
                    <button
                      key={hint.name}
                      type="button"
                      disabled={disabled}
                      className="ws-create-hint-chip"
                      onClick={() => {
                        onChange(hint.name)
                        newInput.current?.focus()
                      }}
                    >
                      <Sparkles size={11} aria-hidden="true" />
                      <strong>{hint.name}</strong>
                      <small>{hint.source}</small>
                    </button>
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
                    disabled={disabled}
                    onClick={() => {
                      onChange(existingExactOrAlias.name)
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
                    <button
                      key={person.name}
                      type="button"
                      disabled={disabled}
                      className="ws-create-hint-chip"
                      onClick={() => {
                        onChange(person.name)
                        setCreating(false)
                      }}
                    >
                      <Avatar name={person.name} src={person.rep_thumb || undefined} size="sm" />
                      <strong>{person.name}</strong>
                    </button>
                  ))}
                </div>
              </div>
            )}

            <div className="ws-create-actions">
              <Button variant="ghost" disabled={disabled} onClick={cancelCreate}>
                <ArrowLeft size={14} aria-hidden="true" />
                返回
              </Button>
              <Button
                variant="primary"
                disabled={disabled || !trimmedValue || !!validationError}
                onClick={commitCreate}
              >
                <UserPlus size={14} aria-hidden="true" />
                {existingExactOrAlias
                  ? `选用 ${existingExactOrAlias.name}`
                  : `使用${trimmedValue ? `「${trimmedValue}」` : '新人物'}`}
              </Button>
            </div>
          </Card>
        ) : (
          <>
            {visibleSamples.length > 0 && (
              <div className="ws-person-sample-strip" role="group" aria-label="当前视频人脸采样">
                {visibleSamples.map((src, idx) =>
                  src && viewerImageIds.has(`sample:${idx}`) ? (
                    <ImageViewerThumbnail
                      key={`sample:${idx}`}
                      imageId={`sample:${idx}`}
                      disabled={disabled}
                      className="ws-person-sample-thumb"
                      imageClassName="ws-person-sample-img"
                    />
                  ) : null,
                )}
              </div>
            )}
            <WorkspaceSearch
              value={query}
              onChange={setQuery}
              label="搜索全部人物"
              placeholder="搜索人物或别名…"
              disabled={disabled}
            />
            {error && (
              <WorkspaceMessage error>
                {error}
                {onRetry && (
                  <Button variant="outline" disabled={disabled || loading} onClick={onRetry}>
                    重试
                  </Button>
                )}
              </WorkspaceMessage>
            )}
            <div className="ws-person-options" role="group" aria-label="可选人物" aria-busy={loading}>
              {loading && !people.length ? (
                <WorkspaceMessage>读取人物…</WorkspaceMessage>
              ) : (
                <div className="ws-person-options-inner">
                  {showSections ? (
                    <>
                      <div className="ws-person-section-label">
                        <span>推荐</span>
                        <span className="ws-count tabular-nums">{recommendedChoices.length}</span>
                      </div>
                      {recommendedChoices.map(renderPersonOption)}
                      <div className="ws-person-section-label">
                        <span>全部人物</span>
                        <span className="ws-count tabular-nums">{otherChoices.length}</span>
                      </div>
                      {otherChoices.map(renderPersonOption)}
                    </>
                  ) : (
                    choices.map(renderPersonOption)
                  )}
                </div>
              )}
            </div>
            {canQuickCreateFromSearch && (
              <Card size="compact" variant="muted" className="ws-quick-create-bar">
                <div className="ws-quick-create-inner">
                  <Button
                    variant="secondary"
                    size="sm"
                    className="w-full justify-center"
                    disabled={disabled}
                    onClick={() => startCreate(trimmedQuery)}
                  >
                    <UserPlus size={13} aria-hidden="true" />
                    新建「{trimmedQuery}」
                  </Button>
                </div>
              </Card>
            )}
            {!loading && !error && !choices.length && !canQuickCreateFromSearch && (
              <WorkspaceMessage>
                没有匹配的人物。
                <Button variant="ghost" disabled={disabled} onClick={() => startCreate()}>
                  新建人物
                </Button>
              </WorkspaceMessage>
            )}
          </>
        )}
      </section>
      <PickerViewerContent
        suggestionMap={suggestionMap}
        catalogMap={catalogMap}
        currentValue={value}
        disabled={disabled}
        onSelectPerson={onChange}
        onComparePerson={name => setComparing(name)}
        onPlayRef={video => setRefPlaying(video)}
        refVideos={currentRef?.videos || []}
      />
    </ImageViewer>
  )
}

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
import { matchesPersonQuery } from '@/lib/person-list-model'
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
import { SharedLayoutBg } from '@/components/beui/motion/shared-layout-bg'
import {
  WorkspaceButton as Button,
  WorkspaceInput,
  WorkspaceField,
  WorkspaceMessage,
  WorkspaceMedia,
  WorkspacePath,
} from '@/components/workbench/inner'

export interface PersonChoice {
  name: string
  rep_thumb: string | null
  aliases?: string[]
  sim?: number
  recent_at?: string
  match_count?: number
  match_reason?: string
}

export interface PersonNameHint {
  name: string
  source: string
}

function normalizeName(text: string) {
  return text.trim().normalize('NFKC').toLowerCase()
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

  // Merge suggestions first so candidates for the current batch stay pinned at the top
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
      ].map(p => [p.name, p])
    ).values(),
  ]

  const choices = combined.filter(p => matchesPersonQuery(p, query))
  const trimmedValue = value.trim()
  const isExistingPerson = !!trimmedValue && catalogMap.has(trimmedValue)
  const isNewPerson = !!trimmedValue && !isExistingPerson
  const selectedPerson = trimmedValue
    ? combined.find(p => p.name === trimmedValue) || {
        name: trimmedValue,
        rep_thumb: sampleFaces[0] || null,
      }
    : null

  // Match check while creating a new person: detect exact or alias match in existing catalog
  const normalizedDraft = normalizeName(value)
  const validationError = creating ? validateNewPersonName(value) : null
  const existingExactOrAlias =
    creating && normalizedDraft && !validationError
      ? people.find(
          p =>
            normalizeName(p.name) === normalizedDraft ||
            (p.aliases || []).some(a => normalizeName(a) === normalizedDraft)
        ) || null
      : null

  const similarExisting =
    creating && normalizedDraft.length >= 2 && !existingExactOrAlias && !validationError
      ? people.filter(p => matchesPersonQuery(p, trimmedValue)).slice(0, 3)
      : []

  // Deduplicate name hints and filter out invalid names
  const validHints = nameHints.filter(
    h => h.name.trim() && !validateNewPersonName(h.name) && h.name.trim() !== trimmedValue
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
      { signal: abort.signal }
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
        : query.trim() || (isNewPerson ? trimmedValue : validHints[0]?.name || '')
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
  const trimmedQuery = query.trim()
  const canQuickCreateFromSearch =
    !creating &&
    !!trimmedQuery &&
    !validateNewPersonName(trimmedQuery) &&
    !people.some(p => normalizeName(p.name) === normalizeName(trimmedQuery))

  return (
    <section className="ws-person-picker" aria-label="归名人物选择">
      <div className="ws-person-choice" data-has-value={!!selectedPerson && !creating}>
        {selectedPerson && !creating ? (
          <Card size="compact" selected className="ws-person-selected-banner">
            <Avatar
              name={selectedPerson.name}
              src={selectedPerson.rep_thumb || sampleFaces[0] || undefined}
              size="md"
              className="ws-person-selected-avatar"
            />
            <div className="ws-person-selected-copy">
              <div className="ws-person-selected-title">
                <strong>{selectedPerson.name}</strong>
                <AnimatedBadge
                  size="sm"
                  status={
                    isNewPerson
                      ? 'success'
                      : suggestionMap.has(selectedPerson.name)
                        ? 'info'
                        : 'neutral'
                  }
                  showIcon={false}
                >
                  {isNewPerson
                    ? '✨ 新人物'
                    : suggestionMap.has(selectedPerson.name)
                      ? '候选'
                      : '已选'}
                </AnimatedBadge>
              </div>
              <small>
                {isNewPerson
                  ? '确认归名后自动创建人物档案'
                  : suggestionMap.get(selectedPerson.name)?.match_reason ||
                    (selectedPerson.aliases?.length
                      ? `别名：${selectedPerson.aliases.slice(0, 2).join(' / ')}`
                      : '已选归名人物')}
              </small>
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
                <Pencil size={12} />
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
              <X size={14} />
            </Button>
          </Card>
        ) : (
          <strong>{creating ? '新建归名人物' : '选择人物'}</strong>
        )}
        {!creating && (
          <Button variant="ghost" disabled={disabled} onClick={() => startCreate()}>
            <Plus size={14} />
            新建人物
          </Button>
        )}
      </div>

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
                  <strong>{comparing} · 对照核对</strong>
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
                      <Check size={12} />
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
                      <Maximize2 size={13} />
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="收起对照视频"
                    title="收起对照"
                    onClick={() => setComparing(null)}
                  >
                    <X size={14} />
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
                    <button
                      key={video.vid}
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
                          <Play size={11} />
                        </span>
                      </div>
                      <span className="ws-inline-reference-name" title={video.name}>
                        {video.name}
                      </span>
                    </button>
                  ))}
                </div>
              ) : (
                <WorkspaceMessage>该人物暂时没有已扫描的对照视频。</WorkspaceMessage>
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
                <strong>{trimmedValue || '未命名新人物'}</strong>
                <small>
                  {sampleFaces.length > 0
                    ? `将使用所选视频的 ${sampleFaces.length} 张人脸建立人物`
                    : '确认归名时自动创建人物目录'}
                </small>
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

          <WorkspaceField label="新人物名称">
            <WorkspaceInput
              ref={newInput}
              value={value}
              disabled={disabled}
              maxLength={120}
              placeholder="输入新人物名（如账号 ID 或昵称）…"
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
              <span className="ws-create-hints-label">线索快填</span>
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
                    <Sparkles size={11} />
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
                  <small>
                    {normalizeName(existingExactOrAlias.name) === normalizedDraft
                      ? '人物库已有同名档案，可直接选用避免重复'
                      : `命中已有别名：${existingExactOrAlias.aliases?.join(' / ')}`}
                  </small>
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
                  <Check size={12} />
                  选用已有
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
                    <small>直接选用</small>
                  </button>
                ))}
              </div>
            </div>
          )}

          <div className="ws-create-actions">
            <Button variant="ghost" disabled={disabled} onClick={cancelCreate}>
              <ArrowLeft size={14} />
              返回人物列表
            </Button>
            <Button
              variant="primary"
              disabled={disabled || !trimmedValue || !!validationError}
              onClick={commitCreate}
            >
              <UserPlus size={14} />
              {existingExactOrAlias
                ? `选用 ${existingExactOrAlias.name}`
                : `使用新人物${trimmedValue ? `「${trimmedValue}」` : ''}`}
            </Button>
          </div>
        </Card>
      ) : (
        <>
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
              <SharedLayoutBg
                inset={0}
                pillClassName="rounded-xl bg-muted/70"
                className="ws-person-options-inner"
              >
                {choices.map(person => {
                  const suggested = suggestionMap.get(person.name)
                  const isComparing = comparing === person.name
                  const isSelected = value === person.name
                  return (
                    <div
                      key={person.name}
                      className="ws-person-option"
                      data-selected={isSelected}
                      data-suggested={!!suggested || undefined}
                    >
                      <Button
                        variant="ghost"
                        className="ws-person-select"
                        disabled={disabled}
                        onClick={() => onChange(person.name)}
                        aria-label={`选择人物 ${person.name}`}
                        aria-pressed={isSelected}
                      >
                        <Avatar
                          name={person.name}
                          src={person.rep_thumb || undefined}
                          size="lg"
                          className="ws-person-avatar"
                        />
                        <span className="ws-person-name">
                          <span className="ws-person-name-row">
                            <span>{person.name}</span>
                            {suggested && (
                              <AnimatedBadge
                                status="success"
                                size="sm"
                                icon={<Sparkles className="size-3" />}
                              >
                                候选
                              </AnimatedBadge>
                            )}
                          </span>
                          {(suggested?.match_reason || person.aliases?.length) && (
                            <small>
                              {suggested?.match_reason ||
                                person.aliases?.slice(0, 2).join(' · ')}
                            </small>
                          )}
                        </span>
                        {isSelected && <Check size={14} />}
                      </Button>
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
                    </div>
                  )
                })}
              </SharedLayoutBg>
            )}
          </div>
          {canQuickCreateFromSearch && (
            <Card size="compact" variant="muted" className="ws-quick-create-bar">
              <div className="ws-quick-create-inner">
                <span className="ws-muted">未找到同名人物？</span>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={disabled}
                  onClick={() => startCreate(trimmedQuery)}
                >
                  <UserPlus size={13} />
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
  )
}

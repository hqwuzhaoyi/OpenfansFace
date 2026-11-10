'use client'
import { useEffect, useRef, useState } from 'react'
import { Check, Eye, Play, Plus, Sparkles, X, ArrowLeft, Maximize2 } from 'lucide-react'
import { matchesPersonQuery } from '@/lib/person-list-model'
import { api } from '@/lib/face-api'
import type { ReviewVideo } from '@/lib/review-ui'
import { VideoCover } from './video-cover'
import { ExternalProfiles } from '@/components/persons/external-profiles'
import { WorkspaceSearch } from '@/components/workbench/controls'
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

export function PersonPicker({
  people,
  suggestions = [],
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
  const selectedPerson = value ? combined.find(p => p.name === value) || { name: value, rep_thumb: null } : null

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

  const startCreate = () => {
    previous.current = value
    setCreating(true)
    onChange(query.trim())
    requestAnimationFrame(() => newInput.current?.focus())
  }
  const cancelCreate = () => {
    onChange(previous.current)
    setCreating(false)
  }

  const toggleCompare = (name: string) => {
    setComparing(current => (current === name ? null : name))
  }

  const currentRef = refState?.name === comparing ? refState : null

  return (
    <section className="ws-person-picker" aria-label="归名人物选择">
      <div className="ws-person-choice" data-has-value={!!value && !creating}>
        {selectedPerson && !creating ? (
          <div className="ws-person-selected-banner">
            {selectedPerson.rep_thumb ? (
              <img
                className="ws-person-selected-avatar"
                data-private-media
                draggable={false}
                src={selectedPerson.rep_thumb}
                alt=""
                loading="lazy"
              />
            ) : (
              <span className="ws-person-selected-avatar ws-person-initial">
                {selectedPerson.name.slice(0, 1)}
              </span>
            )}
            <div className="ws-person-selected-copy">
              <strong>{selectedPerson.name}</strong>
              <small>
                {suggestionMap.get(selectedPerson.name)?.match_reason ||
                  (selectedPerson.aliases?.length
                    ? `别名：${selectedPerson.aliases.slice(0, 2).join(' / ')}`
                    : '已选归名人物')}
              </small>
            </div>
            <Button
              variant={comparing === selectedPerson.name ? 'secondary' : 'outline'}
              size="sm"
              disabled={disabled}
              aria-label={`看 ${selectedPerson.name} 的对照视频`}
              aria-pressed={comparing === selectedPerson.name}
              title="原位展开对照视频核对"
              onClick={() => toggleCompare(selectedPerson.name)}
            >
              <Eye size={13} />
              对照
            </Button>
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
          </div>
        ) : (
          <strong>{creating ? '新人物' : '选择人物'}</strong>
        )}
        {!creating && (
          <Button variant="ghost" disabled={disabled} onClick={startCreate}>
            <Plus size={14} />
            新建人物
          </Button>
        )}
      </div>

      {/* Inline Reference Drawer: compare person's archived videos without leaving selection mode */}
      {comparing && !creating && (
        <div className="ws-inline-reference" role="region" aria-label={`${comparing} 的对照视频`}>
          <div className="ws-inline-reference-head">
            <div className="ws-inline-reference-title">
              <strong>{comparing} · 对照核对</strong>
              {currentRef && <span className="ws-count">{currentRef.total}</span>}
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
        </div>
      )}

      {creating ? (
        <div className="ws-create-person">
          <WorkspaceField label="新人物名称">
            <WorkspaceInput
              ref={newInput}
              value={value}
              disabled={disabled}
              maxLength={120}
              aria-label="归名人物"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="none"
              onChange={e => onChange(e.target.value)}
            />
          </WorkspaceField>
          <Button variant="ghost" disabled={disabled} onClick={cancelCreate}>
            <ArrowLeft size={14} />
            返回人物列表
          </Button>
        </div>
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
              choices.map(person => {
                const suggested = suggestionMap.get(person.name)
                const isComparing = comparing === person.name
                return (
                  <div
                    key={person.name}
                    className="ws-person-option"
                    data-selected={value === person.name}
                    data-suggested={!!suggested || undefined}
                  >
                    <Button
                      variant="ghost"
                      className="ws-person-select"
                      disabled={disabled}
                      onClick={() => onChange(person.name)}
                      aria-label={`选择人物 ${person.name}`}
                      aria-pressed={value === person.name}
                    >
                      {person.rep_thumb ? (
                        <img
                          data-private-media
                          draggable={false}
                          src={person.rep_thumb}
                          alt=""
                          loading="lazy"
                        />
                      ) : (
                        <span className="ws-person-initial">{person.name.slice(0, 1)}</span>
                      )}
                      <span className="ws-person-name">
                        <span className="ws-person-name-row">
                          <span>{person.name}</span>
                          {suggested && (
                            <span className="ws-person-suggest-badge">
                              <Sparkles size={10} />
                              候选
                            </span>
                          )}
                        </span>
                        {(suggested?.match_reason || person.aliases?.length) && (
                          <small>
                            {suggested?.match_reason ||
                              person.aliases?.slice(0, 2).join(' · ')}
                          </small>
                        )}
                      </span>
                      {value === person.name && <Check size={14} />}
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
                      <Eye size={14} />
                    </Button>
                  </div>
                )
              })
            )}
          </div>
          {!loading && !error && !choices.length && (
            <WorkspaceMessage>
              没有匹配的人物。
              <Button variant="ghost" disabled={disabled} onClick={startCreate}>
                新建人物
              </Button>
            </WorkspaceMessage>
          )}
        </>
      )}
    </section>
  )
}

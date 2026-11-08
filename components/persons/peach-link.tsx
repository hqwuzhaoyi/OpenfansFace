'use client'

import { useEffect, useState } from 'react'
import { ExternalLink, RefreshCw } from 'lucide-react'
import { ButtonLink } from '@/components/beui/motion/button/base'
import { api } from '@/lib/face-api'
import { peachPersonHref, type PeachPersonLookup } from '@/lib/person-peach-model'
import { WorkbenchButton as Button } from './ui'

type Result = { name: string; refreshKey: unknown; title: string; href?: string; failed?: boolean }

/** A separate read keeps the directory usable when Peach is unavailable. */
export function PeachPersonLink({ name, refreshKey }: { name: string; refreshKey?: unknown }) {
  const [result, setResult] = useState<Result | null>(null)
  const [attempt, setAttempt] = useState(0)
  useEffect(() => {
    setResult(null)
    if (!name) return
    const controller = new AbortController()
    void api<PeachPersonLookup>(`/api/persons/peach?name=${encodeURIComponent(name)}`, { signal: controller.signal })
      .then(value => {
        if (controller.signal.aborted) return
        if (value.status === 'linked') {
          setResult({ name, refreshKey, href: peachPersonHref(value.person.id, value.base_url, window.location.origin), title: `在 Peach 打开 ${value.person.name}（新标签页）` })
        } else if (value.status === 'missing' || value.status === 'ambiguous') {
          setResult({ name, refreshKey, title: value.status === 'missing' ? 'Peach 尚无对应人物记录' : 'Peach 存在多个匹配人物，需先核对' })
        } else throw new Error('Peach 返回未知状态')
      })
      .catch(() => {
        if (!controller.signal.aborted) setResult({ name, refreshKey, failed: true, title: 'Peach 暂时无法读取，点击重试' })
      })
    return () => controller.abort()
  }, [name, refreshKey, attempt])

  if (!name) return null
  const current = result?.name === name && result.refreshKey === refreshKey ? result : null
  if (current?.href) return <ButtonLink className="pw-peach-link" variant="outline" size="sm" whileTap={undefined} href={current.href} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer" title={current.title} aria-label={current.title}>Peach<ExternalLink size={13}/></ButtonLink>
  const title = current?.title ?? '正在读取 Peach 人物记录'
  return <Button className="pw-peach-link" disabled={!current?.failed} title={title} aria-label={title} onClick={() => { setResult(null); setAttempt(value => value + 1) }}>Peach{current?.failed && <RefreshCw size={13}/>}</Button>
}

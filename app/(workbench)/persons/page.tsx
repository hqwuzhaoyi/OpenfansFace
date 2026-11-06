"use client"

import { useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { api, type Person } from '@/lib/face-api'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { RefreshCw } from 'lucide-react'

type PersonRow = Pick<Person, 'name' | 'rep_thumb'> & Partial<Person>

export default function Persons() {
  const [persons, setPersons] = useState<PersonRow[]>([])
  const [loading, setLoading] = useState(true)
  const [detailsLoading, setDetailsLoading] = useState(false)
  const [error, setError] = useState('')
  const [detailsError, setDetailsError] = useState('')
  const request = useRef<AbortController | null>(null)
  const [q, setQ] = useState('')
  const load = async () => {
    request.current?.abort()
    const controller = new AbortController(); request.current = controller
    setLoading(true); setDetailsLoading(false); setError(''); setDetailsError('')
    try {
      // Names do not depend on a NAS directory walk or the Peach ledger.
      const catalog = await api<{ persons: PersonRow[] }>('/api/persons?picker=1', { signal: controller.signal })
      if (controller.signal.aborted) return
      setPersons(catalog.persons); setLoading(false); setDetailsLoading(true)
      try {
        const details = await api<{ persons: Person[] }>('/api/persons', { signal: controller.signal })
        if (!controller.signal.aborted) setPersons(details.persons)
      } catch (e) {
        if (!controller.signal.aborted) setDetailsError((e as Error).message)
      }
    } catch (e) {
      if (!controller.signal.aborted) setError((e as Error).message)
    } finally {
      if (!controller.signal.aborted) { setLoading(false); setDetailsLoading(false) }
    }
  }
  useEffect(() => { load(); return () => request.current?.abort() }, [])
  const rows = persons.filter((p) => !q || p.name.toLowerCase().includes(q.toLowerCase()))
  return (
    <div className="space-y-4 p-4 md:p-6">
      <div className="flex flex-wrap items-center gap-3">
        <div>
          <h1 className="text-2xl font-bold tracking-tight">人物</h1>
          <p className="text-sm text-muted-foreground">选择人物 → 核对视频 → 确认后加入待归档</p>
        </div>
        <Input placeholder="搜索人物…" value={q} onChange={(e) => setQ(e.target.value)} className="max-w-xs" />
        <Badge variant="outline">{!persons.length && (loading || error) ? loading ? '加载中…' : '未加载' : `${rows.length} 人`}</Badge>
        <Button className="min-h-11" variant="outline" disabled={loading || detailsLoading} onClick={load}><RefreshCw className="h-4 w-4" />刷新人物</Button>
      </div>
      {(error || detailsError) && <div role="alert" className="space-y-3 rounded-xl border border-amber-500/30 bg-amber-500/[0.05] p-4 text-sm">
        <p className="font-medium">{error ? '人物名单加载失败' : '人物已显示，视频统计暂未加载成功'}</p>
        <p className="text-muted-foreground">{error || detailsError}</p>
        {detailsError && <p className="text-muted-foreground">可以先点人物继续核对视频，稍后再刷新统计。</p>}
        <Button className="min-h-11" variant="outline" onClick={load}>重新加载</Button>
      </div>}
      {(loading || detailsLoading) && <p role="status" className="text-sm text-muted-foreground">{loading ? '正在加载人物名单…' : '人物已加载，可以直接点开。正在补充视频数量和目录统计…'}</p>}
      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-12"></TableHead>
              <TableHead>名字</TableHead>
              <TableHead>识别推荐</TableHead>
              <TableHead>目录文件</TableHead>
              <TableHead>Peach 关联</TableHead>
              <TableHead>别名</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {!loading && !error && !rows.length && <TableRow><TableCell colSpan={6} className="py-12 text-center text-muted-foreground">{q ? <><p>没有找到“{q}”</p><Button variant="ghost" className="mt-3 min-h-11" onClick={() => setQ('')}>清空搜索，查看全部人物</Button></> : '当前还没有人物。确认视频归名后，人物会显示在这里。'}</TableCell></TableRow>}
            {rows.map((p) => (
              <TableRow key={p.name}>
                <TableCell>
                  <Avatar className="h-9 w-9 rounded-lg">
                    <AvatarImage src={p.rep_thumb} />
                    <AvatarFallback>{p.name.slice(0, 2)}</AvatarFallback>
                  </Avatar>
                </TableCell>
                <TableCell>
                  <Link href={`/persons/${encodeURIComponent(p.name)}`} className="inline-flex min-h-11 items-center font-medium hover:underline">
                    {p.name}
                  </Link>
                </TableCell>
                <TableCell className="text-muted-foreground">{p.claimed_n === undefined ? '待加载' : `${p.claimed_n} 个 / ${p.claimed_gb}G`}</TableCell>
                <TableCell>
                  {p.integrated_files === undefined ? <span className="text-muted-foreground">待加载</span> : p.integrated_files > 0
                    ? <span className="text-emerald-500">{p.integrated_files} 个 / {p.integrated_gb}G</span>
                    : <span className="text-muted-foreground">—</span>}
                </TableCell>
                <TableCell>{p.peach_links === undefined ? <span className="text-muted-foreground">待加载</span> : <Badge variant={p.peach_links > 0 ? 'default' : 'secondary'}>{p.peach_links}</Badge>}</TableCell>
                <TableCell className="max-w-48 truncate text-xs text-muted-foreground">{p.aliases?.join(' · ')}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  )
}

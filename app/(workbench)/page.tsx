"use client"

import { useEffect, useState } from 'react'
import { api, type Stats } from '@/lib/face-api'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Progress } from '@/components/ui/progress'
import { RefreshCw as IconRefresh } from 'lucide-react'

function Funnel({ label, value, sub, tone }: { label: string; value: number | string; sub?: string; tone?: string }) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">{label}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className={`text-2xl font-bold ${tone ?? ''}`}>{value}</div>
        {sub && <p className="text-xs text-muted-foreground mt-1">{sub}</p>}
      </CardContent>
    </Card>
  )
}

export default function Overview() {
  const [stats, setStats] = useState<Stats | null>(null)
  const [err, setErr] = useState('')
  useEffect(() => {
    api<Stats>('/api/stats').then(setStats).catch((e) => setErr(String(e)))
  }, [])
  if (err) return <div className="p-4 text-destructive">加载失败：{err}</div>
  if (!stats) return <div className="p-4 text-muted-foreground">加载中…</div>
  const f = stats.funnel
  return (
    <div className="space-y-4 p-4 md:p-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold tracking-tight">总览</h1>
          <p className="text-sm text-muted-foreground">人脸策展流水线 · 全库漏斗与待办</p>
        </div>
        {stats.rebuild?.running && (
          <Badge variant="secondary" className="gap-1">
            <IconRefresh className="h-3 w-3 animate-spin" /> 重建中
          </Badge>
        )}
      </div>
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
        <Funnel label="已扫描视频" value={f.scanned.toLocaleString()} />
        <Funnel label="已认领" value={f.claimed.toLocaleString()} sub={`${f.claimed_gb}G · ${stats.creators_dirs} 个人物`} />
        <Funnel label="已整合入库" value={f.integrated_files.toLocaleString()} sub={`${f.integrated_gb}G 进 creators/`} tone="text-emerald-500" />
        <Funnel label="无名聚类" value={f.unknown_clusters} sub={`${f.unknown_n} 个视频 / ${f.unknown_gb}G`} tone="text-amber-500" />
        <Funnel label="My TG 存量" value={f.mytg_remaining.toLocaleString()} sub="未认领，等待下一轮" />
      </div>
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm font-medium text-muted-foreground">识别待确认</CardTitle></CardHeader>
          <CardContent>
            <div className="text-3xl font-bold text-amber-500">{stats.pending_review}</div>
            <p className="text-xs text-muted-foreground mt-1">核对视频和推荐人物，确认后加入待归档</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm font-medium text-muted-foreground">staging 回收站</CardTitle></CardHeader>
          <CardContent>
            <div className="text-3xl font-bold">{stats.staging.files}</div>
            <p className="text-xs text-muted-foreground mt-1">{stats.staging.gb}G · 可随时恢复</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm font-medium text-muted-foreground">mnt2 磁盘</CardTitle></CardHeader>
          <CardContent>
            <div className="text-3xl font-bold">{stats.disk?.used_pct ?? '-'}%</div>
            <Progress value={stats.disk?.used_pct ?? 0} className="mt-2" />
            <p className="text-xs text-muted-foreground mt-1">剩 {stats.disk?.free_gb ?? '-'}G / 共 {stats.disk?.total_gb ?? '-'}G</p>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

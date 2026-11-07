"use client"

import { useEffect, useState } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { VideoHealthPanel } from '@/components/review/video-health-panel'

export default function Staging() {
  const [s, setS] = useState<any>(null)
  useEffect(() => { fetch('/api/staging').then((r) => r.json()).then(setS).catch(() => {}) }, [])
  return (
    <main className="wb-page space-y-4">
      <header className="wb-header">
        <h1 className="wb-title">回收站</h1>
      </header>
      <div className="grid gap-3 sm:grid-cols-2">
        <Card className="p-4">
          <CardHeader className="p-0 pb-1"><CardTitle className="text-xs font-medium text-muted-foreground">暂存文件数</CardTitle></CardHeader>
          <CardContent className="p-0"><div className="text-2xl font-semibold tabular-nums">{s?.files ?? '—'}</div></CardContent>
        </Card>
        <Card className="p-4">
          <CardHeader className="p-0 pb-1"><CardTitle className="text-xs font-medium text-muted-foreground">暂存占用</CardTitle></CardHeader>
          <CardContent className="p-0"><div className="text-2xl font-semibold tabular-nums">{s?.gb ?? '—'} GB</div>{s?.path && <p className="mt-1 truncate text-xs text-muted-foreground" title={s.path}>{s.path}</p>}</CardContent>
        </Card>
      </div>
      {s && !s.files && <p className="text-xs text-muted-foreground">暂存目录当前没有文件。</p>}
      <VideoHealthPanel />
    </main>
  )
}

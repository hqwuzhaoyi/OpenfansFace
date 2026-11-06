"use client"

import { useEffect, useState } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { VideoHealthPanel } from '@/components/review/video-health-panel'

export default function Staging() {
  const [s, setS] = useState<any>(null)
  useEffect(() => { fetch('/api/staging').then((r) => r.json()).then(setS).catch(() => {}) }, [])
  return (
    <div className="space-y-4 p-4 md:p-6">
      <div>
        <h1 className="text-2xl font-bold tracking-tight">回收站</h1>
        <p className="text-sm text-muted-foreground">查看暂存文件和视频体检。自动体检会收起确认异常的记录，原文件保留。</p>
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm font-medium text-muted-foreground">文件数</CardTitle></CardHeader>
          <CardContent><div className="text-3xl font-bold">{s?.files ?? '-'}</div></CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm font-medium text-muted-foreground">占用</CardTitle></CardHeader>
          <CardContent><div className="text-3xl font-bold">{s?.gb ?? '-'}G</div><p className="mt-1 text-xs text-muted-foreground">{s?.path}</p></CardContent>
        </Card>
      </div>
      {s && !s.files && <p className="py-4 text-sm text-muted-foreground">暂存目录当前没有文件。</p>}
      <VideoHealthPanel />
    </div>
  )
}

import { NextResponse } from 'next/server'
import { loadVideoHealth } from '@/lib/video-health'

export async function GET() {
  const report=loadVideoHealth()
  const fresh=!!report.last_success_at&&Date.now()-Date.parse(report.last_success_at)<7*86400_000
  const rows=Object.values(report.records).filter(r=>!['healthy','unchecked','present'].includes(r.status))
    .map(r=>({...r,hidden:!!r.hidden&&fresh}))
    .sort((a,b)=>Number(!!b.hidden)-Number(!!a.hidden)||a.status.localeCompare(b.status)||String(a.path).localeCompare(String(b.path)))
  const {records,...summary}=report
  return NextResponse.json({...summary,hidden:rows.filter(r=>r.hidden).length,rows,...(!fresh&&report.last_success_at?{error:'检查结果超过 7 天，暂停自动收起，等待下一次成功检查。'}:{})})
}

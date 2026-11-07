'use client'
import { useState } from 'react'
import { DonutChart,DonutChartPlot,DonutChartCenter,DonutChartLegend,useDonutChart,type DonutSegment } from '@/components/beui/charts/donut-chart'
import { DonutContext } from '@/components/beui/charts/donut-chart/context'
import { BarChart,BarChartPlot,BarChartTooltip } from './charts-vendor/beui/components/charts/bar-chart'
import { StatusBar,StatusBarPlot,StatusBarTooltip,StatusBarLegend,type StatusBarDatum } from './charts-vendor/beui/components/charts/status-bar'
export type { StatusBarDatum }
export const chartStates=[
 {id:'done',name:'分析完成',color:'var(--wb-success)'},
 {id:'running',name:'运行中',color:'var(--primary)'},
 {id:'waiting',name:'排队中',color:'var(--wb-border-strong)'},
 {id:'error',name:'异常',color:'var(--wb-danger)'},
 {id:'excluded',name:'已移出',color:'var(--wb-warning)'},
] as const
const count=(n:number)=>n.toLocaleString('zh-CN')
const percent=(n:number,total:number)=>`${(total?n/total*100:0).toLocaleString('zh-CN',{maximumFractionDigits:2})}%`
function DistributionParts({complete,total}:{complete:number;total:number}){
 const model=useDonutChart()
 // Trigonometric last-bit differences between Node and Chromium must not alter SSR SVG attributes.
 const placed=model.placed.map(segment=>({...segment,path:segment.path.replace(/-?\d*\.?\d+(?:e[-+]?\d+)?/gi,value=>String(Number(Number(value).toFixed(6))))}))
 // Reuse the official plot/legend and their inspection state, with immediate dashboard updates.
 return <DonutContext.Provider value={{...model,placed,reduce:true}}><div className="bm-distribution-layout"><DonutChartPlot><DonutChartCenter>{segment=><><strong className="bm-ring-value">{percent(segment?.value??complete,total)}</strong><span className="bm-ring-label">{segment?.name||'分析完成'}</span></>}</DonutChartCenter></DonutChartPlot><DonutChartLegend renderValue={s=><span>{count(s.value)}</span>}/></div></DonutContext.Provider>
}
export function TaskDistribution({complete,running,waiting,errors,total,excluded=0}:{complete:number;running:number;waiting:number;errors:number;total:number;excluded?:number}){
 const [hover,setHover]=useState<string|null>(null),[pinned,setPinned]=useState<string|null>(null)
 const values=[complete,running,waiting,errors,excluded]
 const segments:DonutSegment[]=chartStates.map((s,i)=>({...s,value:values[i]}))
 return <DonutChart segments={segments} label="任务分布：分析完成、运行中、排队中、异常" thickness={21} gap={2} activeId={hover||pinned} onActiveChange={setHover} selectedId={pinned} onSelect={id=>setPinned(previous=>previous===id?null:id)} formatValue={count}><DistributionParts complete={complete} total={total}/></DonutChart>
}
export function BatchStack({name,total,complete,running,errors,excluded=0}:{name:string;total:number;complete:number;running:number;errors:number;excluded?:number}){
 const values=[complete,running,total-complete-running-errors-excluded,errors,excluded]
 return <BarChart className="bm-inline-stack" categories={[name]} series={chartStates.map((s,i)=>({...s,values:[values[i]]}))} orientation="horizontal" layout="stacked" max={total} label={`${name}任务构成`} formatValue={n=>`${count(n)} 个`} formatTick={()=>''}><BarChartPlot height={38} labelWidth={0} showGrid={false} className="bm-inline-stack-plot"/><BarChartTooltip>{({category,rows})=><div className="bm-stack-tooltip"><strong>{category}</strong>{rows.map(row=><div key={row.id}><span><i style={{background:row.color}}/>{row.name}</span><b>{row.value===null?'未知':count(row.value)} 个</b></div>)}<footer>本批总量 {count(total)} 个</footer></div>}</BarChartTooltip></BarChart>
}
export function StackLegend(){return <div className="bm-stack-legend" aria-label="任务状态图例">{chartStates.map(s=><span key={s.id}><i style={{backgroundColor:s.color}}/>{s.name}</span>)}</div>}
const historyStates=[{id:'running',label:'执行中',color:'var(--primary)'},{id:'interrupted',label:'中断',color:'var(--wb-warning)'},{id:'cancelled',label:'已取消',color:'var(--muted-foreground)'},{id:'unknown',label:'未知',color:'var(--muted-foreground)'},{id:'done',label:'完成',color:'var(--wb-success)'},{id:'error',label:'失败',color:'var(--wb-danger)'},{id:'retry',label:'重试',color:'var(--wb-warning)'}]
export function RunHistory({data,compact=false}:{data:StatusBarDatum[];compact?:boolean}){return <StatusBar data={data} statuses={historyStates.filter(s=>data.some(d=>d.status===s.id))} label="最近阶段执行记录" className={`bm-run-history ${compact?'bm-run-history-compact':''}`}><StatusBarPlot showLabels={!compact} barClassName={compact?'h-5 rounded-sm':'h-6 rounded-md'}/><StatusBarTooltip/><StatusBarLegend showCounts/></StatusBar>}

import { peopleCatalog } from '@/lib/people-catalog'
import { reviewScope } from "@/lib/review-scope"
import { resolvedVids } from "@/lib/review-store"
import { NextRequest, NextResponse } from "next/server"
import { loadReport, integratedStatsAsync, peachPersons } from "@/lib/integration"

export async function GET(req:NextRequest) {
  if(req.nextUrl.searchParams.get("picker")==="1")return NextResponse.json({persons:peopleCatalog()})
  const r = loadReport(), scope=reviewScope(), handled=resolvedVids()
  let creators, links, aliases
  try {
    [creators, {links,aliases}] = await Promise.all([integratedStatsAsync(),peachPersons()])
  } catch {
    return NextResponse.json({error:'人物统计暂时无法读取，请稍后重试。人物名单仍可使用。'},{status:503})
  }
  const models=new Map((r.identities||[]).map((i:any)=>[i.name,i]))
  const out = peopleCatalog().map((person) => {
    const i:any=models.get(person.name)||{name:person.name,vids:[],aliases:[],tpl_faces:0}
    const pending=(i.vids||[]).filter((v:any)=>!handled.has(v.vid)&&scope.eligible(v.vid))
    const files = creators[i.name] || { files: 0, gb: 0 }
    return {
      name: i.name, type: "creator",
      claimed_n: pending.length, claimed_gb: +(pending.reduce((n:number,v:any)=>n+v.size,0)/2**30).toFixed(1),
      integrated_files: files.files, integrated_gb: files.gb,
      tpl_faces: i.tpl_faces ?? 0, cohesion: i.cohesion ?? null,
      peach_entity: true, peach_links: links[i.name] ?? 0,
      aliases: aliases[i.name] ?? (i.aliases || []).slice(0, 3),
      rep_thumb: person.rep_thumb, recent_at:person.recent_at,
    }
  })
  out.sort((a: any, b: any) => b.integrated_gb - a.integrated_gb)
  return NextResponse.json({ persons: out, stats: r.stats ?? {} })
}

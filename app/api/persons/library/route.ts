import { NextRequest, NextResponse } from 'next/server'
import { libraryPeople, personLibrary } from '@/lib/person-library'
import { personDirectoryStats } from '@/lib/person-library-stats'
import { canonicalPersonName } from '@/lib/person-names'
import { validateCreator } from '@/lib/review-logic'
import { cachedRedisJson, getRedisCacheEpoch, redisSetJson } from '@/lib/redis'

const NO_STORE_HEADERS = { 'Cache-Control': 'no-store' }

function hasStatsError(result: Awaited<ReturnType<typeof personDirectoryStats>> | null | undefined): boolean {
  if (!result || typeof result.stats !== 'object' || result.stats === null) return true
  return Object.values(result.stats).some((entry) => Boolean(entry?.stats_error))
}

export async function GET(req:NextRequest) {
  try {
    const refresh=req.nextUrl.searchParams.get('refresh')==='1'
    if(req.nextUrl.searchParams.get('stats')==='1'){
      if(refresh){
        const epoch=getRedisCacheEpoch()
        const fresh=await personDirectoryStats(true)
        if(!hasStatsError(fresh)&&epoch===getRedisCacheEpoch())await redisSetJson('persons:library:stats',fresh,300)
        return NextResponse.json(fresh,{headers:NO_STORE_HEADERS})
      }
      return NextResponse.json(await cachedRedisJson('persons:library:stats',300,()=>personDirectoryStats(false),(result)=>!hasStatsError(result)),{headers:NO_STORE_HEADERS})
    }
    const name=req.nextUrl.searchParams.get('name')
    if(name!==null&&req.nextUrl.searchParams.get('preview')==='1'){
      const offset=Number(req.nextUrl.searchParams.get('offset')||0)
      if(!Number.isSafeInteger(offset)||offset<0)return NextResponse.json({error:'分页参数无效'},{status:400,headers:NO_STORE_HEADERS})
      const library=await personLibrary(name,false),files=library.files.slice(offset,offset+4)
      return NextResponse.json({name:library.name,root:library.root,total_videos:library.files.length,total_bytes:library.files.reduce((sum,file)=>sum+file.size,0),files,next_offset:offset+files.length<library.files.length?offset+files.length:null},{headers:NO_STORE_HEADERS})
    }
    if(name!==null){
      let canonicalName:string|null=null
      try{canonicalName=canonicalPersonName(validateCreator(name))}catch{/* Fall through to personLibrary(name) error behavior */}
      if(!canonicalName)return NextResponse.json(await personLibrary(name),{headers:NO_STORE_HEADERS})
      if(refresh){
        const epoch=getRedisCacheEpoch()
        const fresh=await personLibrary(name)
        if(epoch===getRedisCacheEpoch())await redisSetJson(`persons:library:detail:${canonicalName}`,fresh,120)
        return NextResponse.json(fresh,{headers:NO_STORE_HEADERS})
      }
      return NextResponse.json(await cachedRedisJson(`persons:library:detail:${canonicalName}`,120,()=>personLibrary(name)),{headers:NO_STORE_HEADERS})
    }
    if(refresh){
      const epoch=getRedisCacheEpoch()
      const fresh={persons:await libraryPeople()}
      if(epoch===getRedisCacheEpoch())await redisSetJson('persons:library:list',fresh,180)
      return NextResponse.json(fresh,{headers:NO_STORE_HEADERS})
    }
    return NextResponse.json(await cachedRedisJson('persons:library:list',180,async()=>({persons:await libraryPeople()})),{headers:NO_STORE_HEADERS})
  }catch(error:any){return NextResponse.json({error:error.message||'人物目录读取失败'},{status:error.status||500,headers:NO_STORE_HEADERS})}
}

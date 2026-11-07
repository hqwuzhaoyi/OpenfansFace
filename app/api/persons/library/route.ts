import { NextRequest, NextResponse } from 'next/server'
import { libraryPeople, personLibrary } from '@/lib/person-library'
import { personDirectoryStats } from '@/lib/person-library-stats'

export async function GET(req:NextRequest) {
  try {
    if(req.nextUrl.searchParams.get('stats')==='1')return NextResponse.json(await personDirectoryStats(req.nextUrl.searchParams.get('refresh')==='1'),{headers:{'Cache-Control':'no-store'}})
    const name=req.nextUrl.searchParams.get('name')
    if(name!==null&&req.nextUrl.searchParams.get('preview')==='1'){
      const offset=Number(req.nextUrl.searchParams.get('offset')||0)
      if(!Number.isSafeInteger(offset)||offset<0)return NextResponse.json({error:'分页参数无效'},{status:400})
      const library=await personLibrary(name,false),files=library.files.slice(offset,offset+4)
      return NextResponse.json({name:library.name,root:library.root,total_videos:library.files.length,total_bytes:library.files.reduce((sum,file)=>sum+file.size,0),files,next_offset:offset+files.length<library.files.length?offset+files.length:null},{headers:{'Cache-Control':'no-store'}})
    }
    return NextResponse.json(name!==null?await personLibrary(name):{persons:await libraryPeople()})
  }catch(error:any){return NextResponse.json({error:error.message||'人物目录读取失败'},{status:error.status||500})}
}

import { nativeMetadataEnabled, nativeMetadata } from '@/lib/shared-metadata-backend'
import fs from 'node:fs'
import { randomUUID } from 'node:crypto'
import { NextRequest, NextResponse } from 'next/server'
import { PERSON_REJECTIONS, loadPersonRejections, rejectionKey, personLibrary } from '@/lib/person-library'
import { acquireReviewLock } from '@/lib/review-store'
import { writeJson } from '@/lib/jsonio'
import { ReviewError, validateCreator, validateVids } from '@/lib/review-logic'
import { canonicalPersonName } from '@/lib/person-names'

export async function POST(req:NextRequest) {
  let release:(()=>void)|undefined
  try {
    const body=await req.json(),name=canonicalPersonName(validateCreator(body.name)),vid=validateVids([body.vid])[0]
    if(!nativeMetadataEnabled())release=acquireReviewLock()
    const video=(await personLibrary(name)).recommendations.find(v=>v.vid===vid)
    if(!video)throw new ReviewError('推荐已变化，请刷新后核对',409)
    const stat=fs.statSync(video.path,{bigint:true}),rows=loadPersonRejections(),id=randomUUID()
    if(nativeMetadataEnabled())return NextResponse.json(await nativeMetadata('recommendation-reject',{name,vid,path:video.path,size:Number(stat.size),mtime_ns:stat.mtimeNs.toString()}))
    rows[rejectionKey(name,vid)]={id,name,vid,path:video.path,size:Number(stat.size),mtime_ns:stat.mtimeNs.toString(),at:new Date().toISOString()}
    writeJson(PERSON_REJECTIONS,rows)
    return NextResponse.json({ok:true,id})
  }catch(error:any){return NextResponse.json({error:error.message,code:error.code,operation_id:error.operation_id},{status:error.status||500})}
  finally{release?.()}
}
export async function DELETE(req:NextRequest) {
  let release:(()=>void)|undefined
  try {
    const body=await req.json(),name=canonicalPersonName(validateCreator(body.name)),vid=validateVids([body.vid])[0]
    if(!nativeMetadataEnabled())release=acquireReviewLock()
    if(nativeMetadataEnabled())return NextResponse.json(await nativeMetadata('recommendation-undo',{name,vid,rejection_id:body.id}))
    const rows=loadPersonRejections(),key=rejectionKey(name,vid)
    if(!body.id||rows[key]?.id!==body.id)throw new ReviewError('这条推荐的处理记录已变化，请刷新',409)
    delete rows[key];writeJson(PERSON_REJECTIONS,rows)
    return NextResponse.json({ok:true})
  }catch(error:any){return NextResponse.json({error:error.message,code:error.code,operation_id:error.operation_id},{status:error.status||500})}
  finally{release?.()}
}

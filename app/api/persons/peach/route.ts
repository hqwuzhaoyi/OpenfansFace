import { NextRequest, NextResponse } from 'next/server'
import { lookupPeachPerson } from '@/lib/person-peach'

export const runtime='nodejs'

export async function GET(req:NextRequest){
  const params=req.nextUrl.searchParams
  const refresh=params.get('refresh')==='1'||params.get('force')==='1'
  try{return NextResponse.json(await lookupPeachPerson(params.get('name')||'',{refresh}),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.status?error.message:'Peach 暂时无法读取，请重试'},{status:error.status||503,headers:{'Cache-Control':'no-store'}})}
}

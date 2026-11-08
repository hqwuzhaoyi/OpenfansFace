import { NextRequest, NextResponse } from 'next/server'
import { lookupPeachPerson } from '@/lib/person-peach'

export const runtime='nodejs'

export async function GET(req:NextRequest){
  try{return NextResponse.json(await lookupPeachPerson(req.nextUrl.searchParams.get('name')||''),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.status?error.message:'Peach 暂时无法读取，请重试'},{status:error.status||503,headers:{'Cache-Control':'no-store'}})}
}

import { NextRequest, NextResponse } from 'next/server'
import { loadPersonAccounts, associatePersonAccount, unlinkPersonAccount } from '@/lib/person-accounts'
import { canonicalPersonName } from '@/lib/person-names'
import { validateCreator } from '@/lib/review-logic'
import { cachedCreatorProfiles } from '@/lib/creator-directory'
const headers={'Cache-Control':'no-store'}
export async function GET(req:NextRequest){
  try{const name=canonicalPersonName(validateCreator(req.nextUrl.searchParams.get('name'))),profiles=new Map(cachedCreatorProfiles().map(profile=>[profile.id,profile]));return NextResponse.json({name,links:loadPersonAccounts().filter(link=>link.creator===name).map(link=>({...link,profile:profiles.get(link.profile.id)||link.profile}))},{headers})}
  catch(error:any){return NextResponse.json({error:error.message},{status:error.status||500,headers})}
}
export async function POST(req:NextRequest){
  try{return NextResponse.json({ok:true,link:await associatePersonAccount(await req.json())},{headers})}
  catch(error:any){return NextResponse.json({error:error.message},{status:error.status||500,headers})}
}
export async function DELETE(req:NextRequest){
  try{unlinkPersonAccount(await req.json());return NextResponse.json({ok:true},{headers})}
  catch(error:any){return NextResponse.json({error:error.message},{status:error.status||500,headers})}
}

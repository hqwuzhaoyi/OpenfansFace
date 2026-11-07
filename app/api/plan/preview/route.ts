import { nativeArchiveEnabled, nativeArchive } from '@/lib/archive-native-backend'
import { NextResponse } from 'next/server'
import { buildPlan } from '@/lib/review-plan'
import { loadArchive } from '@/lib/review-store'
export async function GET() {
  try {if(nativeArchiveEnabled())return NextResponse.json(await nativeArchive('archive-preview'),{headers:{'Cache-Control':'no-store'}})}
  catch(error:any){return NextResponse.json({error:error.message,code:error.code,path:error.path,step:error.step},{status:error.status||500})}
  return NextResponse.json({ ...buildPlan(), history: loadArchive().slice().reverse().slice(0, 200) })
}

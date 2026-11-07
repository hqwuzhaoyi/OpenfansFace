import { redirect } from 'next/navigation'

export default async function PreviousReview({ searchParams }: { searchParams: Promise<{ vid?: string; cid?: string }> }) {
  const params = await searchParams, query = new URLSearchParams()
  if(params.vid)query.set('vid',params.vid)
  if(params.cid)query.set('cluster',params.cid)
  redirect('/review'+(query.size?'?'+query.toString():''))
}

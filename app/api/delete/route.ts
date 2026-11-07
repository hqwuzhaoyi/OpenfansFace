import { NextResponse } from 'next/server'

// This obsolete route bypassed preview, the shared lock, and Peach synchronization.
export async function POST() {
  return NextResponse.json({error:'此删除入口已停用，请通过人物库预览并移入全局暂存区'}, {status:410})
}

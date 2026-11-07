import { NextResponse } from 'next/server'

// Old manifests lack the native before-image and ownership proof required to recover.
// Historical receipts remain readable; never replay overwrite-capable legacy moves.
export async function POST() {
  return NextResponse.json({ error: '旧版回滚执行入口已退役；请按原操作编号核对回执并使用原生恢复流程', code: 'legacy_writer_retired' }, { status: 410 })
}

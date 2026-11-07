import { NextResponse } from 'next/server'
import { peachExec } from '@/lib/peach'

let pending: Promise<any> | undefined
let cached: { at: number; value: any } | undefined
export async function GET() {
  try {
    if (cached && Date.now() - cached.at < 30_000) return NextResponse.json(cached.value)
    if (!pending) pending = (async () => {
      const result = await peachExec(`import sqlite3,json,signal
signal.alarm(30)
source=sqlite3.connect('file:/data/database/ledger.db?mode=ro',uri=True,timeout=15)
source.row_factory=sqlite3.Row
try:
 # One SELECT already has a consistent SQLite read snapshot. Copying the
 # entire ledger into RAM here duplicates unrelated metadata on every poll.
 rows=[dict(r) for r in source.execute("SELECT id,path,name,size,duration,disposal,mtime FROM asset WHERE medium='video'")]
finally:
 source.close()
print(json.dumps({'assets':rows},ensure_ascii=False))`)
      if (result.code !== 0) throw new Error(result.err || '媒体账本快照失败')
      const value = JSON.parse(result.out)
      cached = { at: Date.now(), value }
      return value
    })().finally(() => { pending = undefined })
    return NextResponse.json(await pending)
  } catch (e:any) { return NextResponse.json({error:e.message},{status:503}) }
}

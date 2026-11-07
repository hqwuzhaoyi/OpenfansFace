import { NextRequest, NextResponse } from "next/server"
import fs from "fs"
import { MANIFESTS_D } from "@/lib/env"
import { acquireReviewLock } from "@/lib/review-store"
import { peachExec } from "@/lib/peach"

export async function POST(req: NextRequest) {
  let release:ReturnType<typeof acquireReviewLock>|undefined
  try{
  release=acquireReviewLock()
  const mf = ((await req.json()).manifest as string) || ""
  if (!mf.startsWith(MANIFESTS_D) || !fs.existsSync(/*turbopackIgnore: true*/ mf)) return NextResponse.json({ error: "bad manifest" }, { status: 400 })
  const m = JSON.parse(fs.readFileSync(/*turbopackIgnore: true*/ mf, "utf8"))
  let undone = 0
  for (const mv of [...(m.moves || [])].reverse()) {
    if (fs.existsSync(/*turbopackIgnore: true*/ mv.dst)) {
      try { fs.renameSync(/*turbopackIgnore: true*/ mv.dst, mv.src); undone++ } catch { }
    }
  }
  if (m.ledger) {
    const rows = m.ledger.map((x: any) => ({ from: x.to, to: x.from }))
    const data = JSON.stringify(rows)
    const script = `import json, sqlite3, pathlib
rows = json.loads(pathlib.Path('/tmp/rr.json').read_text())
con = sqlite3.connect('/data/database/ledger.db', timeout=120)
n = 0
for r in rows:
    n += con.execute("UPDATE asset SET path=? WHERE path=?", (r['to'], r['from'])).rowcount
con.commit()
print(n)`
    const b64 = Buffer.from(data).toString("base64")
    const up = await peachExec(`import pathlib,base64; pathlib.Path('/tmp/rr.json').write_bytes(base64.b64decode('${b64}'))`)
    if (up.code === 0) await peachExec(script)
  }
  return NextResponse.json({ ok: true, undone })
  }catch(error:any){return NextResponse.json({error:error.message||'恢复操作失败'},{status:error.status||500})}
  finally{release?.()}
}

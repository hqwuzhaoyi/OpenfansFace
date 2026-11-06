import fs from "fs"
import { randomUUID } from "crypto"
import { QUEUE_F, DECISIONS_F, OVR_F } from "./env"

export function readJson<T = any>(p: string, fallback: T): T {
  try { return JSON.parse(fs.readFileSync(/*turbopackIgnore: true*/ p, "utf8")) as T } catch { return fallback }
}
export function writeJson(p: string, data: any) {
  const tmp = `${p}.${randomUUID()}.tmp`
  try {
    fs.writeFileSync(/*turbopackIgnore: true*/ tmp, JSON.stringify(data, null, 1), "utf8")
    fs.renameSync(/*turbopackIgnore: true*/ tmp, /*turbopackIgnore: true*/ p)
  } finally { fs.rmSync(/*turbopackIgnore: true*/ tmp, { force: true }) }
}
export const loadQueue = () => readJson<any[]>(QUEUE_F, [])
export const saveQueue = (q: any[]) => writeJson(QUEUE_F, q)
export const loadDecisions = () => readJson<{ approved: string[]; rejected: string[] }>(DECISIONS_F, { approved: [], rejected: [] })
export const saveDecisions = (d: any) => writeJson(DECISIONS_F, d)
export const loadOverride = () => readJson<Record<string, any>>(OVR_F, {})
export const saveOverride = (o: any) => writeJson(OVR_F, o)

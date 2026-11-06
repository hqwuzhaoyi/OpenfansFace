import Database from "better-sqlite3"
import { BASE } from "./env"

export function facesDb() {
  return new Database(`${BASE}/faces.db`, { readonly: false, fileMustExist: true })
}

/** thumbs path helpers */
export function thumbUrl(p: string | null | undefined): string | null {
  return p ? `/thumb?p=${p}` : null
}

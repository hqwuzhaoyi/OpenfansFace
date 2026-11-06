import { DRIVE, MEDIA2 } from "./env"

/** windows ledger path R:\media\pikpak\X -> /mnt/media2/pikpak/X */
export function diskView(p: string): string | null {
  if (!p.includes("\\") && !p.startsWith(DRIVE)) return null
  const mapped = p.startsWith(DRIVE) ? p.slice(2).replace(/\\/g, "/") : p.replace(/\\/g, "/")
  const i = mapped.indexOf("media/pikpak/")
  if (i === -1) return null
  return `${MEDIA2}/pikpak/` + mapped.slice(i + "media/pikpak/".length)
}

/** /mnt/media2/pikpak/X -> R:\media\pikpak\X  (apply manifest win path) */
export function toWin(p: string): string {
  const i = p.indexOf("pikpak/")
  return DRIVE + "\\media\\" + p.slice(i).replace(/\//g, "\\")
}

"use client"

import { useEffect } from "react"
import { HomeShell } from "@/components/home/shell"

export default function WorkbenchLayout({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    const keyboard = () => { document.documentElement.dataset.input = "keyboard" }
    const pointer = () => { document.documentElement.dataset.input = "pointer" }
    window.addEventListener("keydown", keyboard, true)
    window.addEventListener("pointerdown", pointer, true)
    return () => {
      window.removeEventListener("keydown", keyboard, true)
      window.removeEventListener("pointerdown", pointer, true)
    }
  }, [])
  return <HomeShell>{children}</HomeShell>
}

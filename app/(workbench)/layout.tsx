"use client"

import { useEffect, useState } from "react"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { HomeShell } from "@/components/home/shell"
import { BackgroundOperationsProvider } from "@/components/workbench/background-operations"

let sharedQueryClient: QueryClient | null = null
function getQueryClient() {
  if (!sharedQueryClient) {
    sharedQueryClient = new QueryClient({
      defaultOptions: {
        queries: {
          staleTime: 15_000,
          gcTime: 30 * 60_000,
          refetchOnWindowFocus: false,
        },
      },
    })
  }
  return sharedQueryClient
}

export default function WorkbenchLayout({ children }: { children: React.ReactNode }) {
  const [client] = useState(getQueryClient)
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
  return (
    <QueryClientProvider client={client}>
      <BackgroundOperationsProvider>
        <HomeShell>{children}</HomeShell>
      </BackgroundOperationsProvider>
    </QueryClientProvider>
  )
}

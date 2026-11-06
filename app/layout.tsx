import type { Metadata, Viewport } from "next"
import "./globals.css"
import { Toaster } from "sonner"

export const metadata: Metadata = {
  title: "OpenfansFace · 视频人物识别与归档",
  description: "视频人物核对、批量归名与归档工作台",
}

export const viewport: Viewport = { width: "device-width", initialScale: 1, viewportFit: "cover", themeColor: "#09090b" }

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="zh-CN" className="dark" suppressHydrationWarning>
      <body className="antialiased">
        {children}
        <Toaster richColors position="bottom-right" theme="dark" />
      </body>
    </html>
  )
}

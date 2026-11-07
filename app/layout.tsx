import type { Metadata, Viewport } from "next"
import "./globals.css"
import { ThemeProvider } from "@/components/theme-provider"

export const metadata: Metadata = {
  title: "OpenfansFace · 视频人物识别与归档",
  description: "视频人物核对、批量归名与归档工作台",
}

export const viewport: Viewport = { width: "device-width", initialScale: 1, viewportFit: "cover" }

const themeInitScript = `(()=>{try{const s=localStorage.getItem('wb-theme')||localStorage.getItem('persons-theme')||'dark';const m=s==='light'||s==='dark'||s==='system'?s:'dark';const r=m==='system'?(window.matchMedia('(prefers-color-scheme: light)').matches?'light':'dark'):m;const d=document.documentElement;d.dataset.theme=r;d.dataset.themeMode=m;d.classList.toggle('dark',r==='dark');d.classList.toggle('light',r==='light');d.style.colorScheme=r;}catch{}})();`

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="zh-CN" className="dark" data-theme="dark" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
      <body className="antialiased">
        <ThemeProvider>{children}</ThemeProvider>
      </body>
    </html>
  )
}

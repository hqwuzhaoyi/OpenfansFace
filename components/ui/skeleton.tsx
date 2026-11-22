// ReUI / shadcn/ui Skeleton (MIT License, see components/ui/SHADCN-LICENSE.md).
// Source: https://reui.io/components/skeleton & https://ui.shadcn.com/r/styles/new-york-v4/skeleton.json
// Uses app/globals.css semantic --accent surface (composite-only opacity pulse) and respects prefers-reduced-motion.

import type * as React from "react"
import { cn } from "@/lib/utils"

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      aria-hidden="true"
      className={cn(
        "animate-pulse motion-reduce:animate-none rounded-md bg-accent",
        className,
      )}
      {...props}
    />
  )
}

export { Skeleton }

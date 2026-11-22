"use client";
// beui.dev/components/motion/dock (MIT License, see components/beui/LICENSE)

import { motion, useReducedMotion } from "motion/react";
import { createContext, useContext, useId, useMemo, type ReactNode } from "react";
import { SPRING_LAYOUT } from "@/lib/beui/ease";
import { cn } from "@/lib/beui/utils";

type DockContextValue = {
  size: number;
  pillLayoutId: string;
};

const DockContext = createContext<DockContextValue | null>(null);

export interface DockProps {
  children: ReactNode;
  className?: string;
  /** Size of each item in px. */
  size?: number;
  orientation?: "horizontal" | "vertical";
}

export function Dock({
  children,
  size = 44,
  orientation = "horizontal",
  className,
}: DockProps) {
  const pillLayoutId = useId();
  const ctx = useMemo<DockContextValue>(
    () => ({ size, pillLayoutId }),
    [size, pillLayoutId],
  );

  return (
    <DockContext.Provider value={ctx}>
      <div
        data-orientation={orientation}
        className={cn(
          "inline-flex h-auto rounded-2xl border border-border bg-card/80 shadow-2xl backdrop-blur-xl",
          orientation === "vertical"
            ? "flex-col items-center gap-1 px-1 py-1.5"
            : "items-end gap-1.5 px-2 py-1",
          className,
        )}
      >
        {children}
      </div>
    </DockContext.Provider>
  );
}

export interface DockItemProps {
  children: ReactNode;
  className?: string;
  /** When set, the item renders as a <button>. Omit when children carry their own link or button. */
  onClick?: () => void;
  active?: boolean;
  /** Allow variable-width content inside the dock item instead of a fixed square. */
  autoWidth?: boolean;
  "aria-label"?: string;
}

export function DockItem({
  children,
  className,
  onClick,
  active,
  autoWidth,
  ...rest
}: DockItemProps) {
  const dock = useContext(DockContext);
  const reduce = useReducedMotion();
  const size = dock?.size ?? 44;
  const pillLayoutId = dock?.pillLayoutId ?? "dock-pill";

  const pill = active ? (
    <motion.span
      layoutId={pillLayoutId}
      transition={reduce ? { duration: 0 } : SPRING_LAYOUT}
      className="absolute inset-0.5 -z-10 rounded-xl bg-muted/60"
    />
  ) : null;
  const sharedStyle = autoWidth ? { minWidth: size, height: size } : { width: size, height: size };
  const sharedClass = cn(
    "relative flex shrink-0 items-center justify-center rounded-full text-foreground",
    className,
  );

  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        aria-label={rest["aria-label"]}
        aria-pressed={active}
        style={sharedStyle}
        className={cn(
          sharedClass,
          "cursor-pointer border-0 bg-transparent p-0 outline-none",
          "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
        )}
      >
        {pill}
        {children}
      </button>
    );
  }

  // Children carry their own link or button (and its accessible name).
  return (
    <div style={sharedStyle} className={sharedClass}>
      {pill}
      {children}
    </div>
  );
}

export function DockSeparator({
  orientation = "horizontal",
  className,
}: {
  orientation?: "horizontal" | "vertical";
  className?: string;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "self-center bg-border",
        orientation === "vertical"
          ? "my-0.5 h-px w-5"
          : "mx-1 h-6 w-px",
        className,
      )}
    />
  );
}

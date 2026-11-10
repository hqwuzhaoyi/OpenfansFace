"use client";

import type { ReactNode } from "react";
import { TextShimmer } from "@/components/beui/motion/text-shimmer";
import { useMessages } from "@/lib/beui/i18n";
import { cn } from "@/lib/beui/utils";
import { type LoadingStatesMessages, loadingStatesMessages } from "./messages";

export interface ThinkingShimmerProps {
  /** Loading message shown to the user. */
  children?: ReactNode;
  /** Seconds taken for one shimmer pass. */
  duration?: number;
  /** Override any built-in string. */
  labels?: Partial<LoadingStatesMessages>;
  className?: string;
}

export function ThinkingShimmer({
  children,
  duration = 1.8,
  labels,
  className,
}: ThinkingShimmerProps) {
  const t = useMessages("loadingStates", loadingStatesMessages, labels);

  return (
    <TextShimmer
      as="span"
      duration={duration}
      className={cn("font-medium", className)}
    >
      {children ?? t("thinking")}
    </TextShimmer>
  );
}

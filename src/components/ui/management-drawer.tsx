"use client";

import type { ComponentProps } from "react";
import { DialogContent } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

type ManagementDrawerContentProps = ComponentProps<typeof DialogContent> & {
  width?: "md" | "lg";
};

/** Shared right-side editor panel for scoped management workflows. */
export function ManagementDrawerContent({
  className,
  width = "md",
  ...props
}: ManagementDrawerContentProps) {
  return (
    <DialogContent
      {...props}
      className={cn(
        "!fixed !inset-y-0 !right-0 !left-auto !top-0 !h-dvh !max-h-none !w-full !translate-x-0 !translate-y-0 !rounded-none sm:!rounded-l-lg !overflow-y-auto !p-5 !animate-in !slide-in-from-right-4 !duration-200",
        width === "lg" ? "!max-w-2xl" : "!max-w-lg",
        className,
      )}
    />
  );
}

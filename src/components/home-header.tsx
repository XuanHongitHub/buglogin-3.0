"use client";

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentOS } from "@/lib/browser-utils";
import { cn } from "@/lib/utils";

const HOLD_MS = 150;
const DRAG_THRESHOLD_PX = 3;

// Any interactive control (buttons, links, inputs) must NOT start a window
// drag when pressed — otherwise the press-and-hold / drag-threshold logic below
// hands the pointer to the OS window-move loop and the control's click never fires.
const isInteractiveTarget = (target: EventTarget | null): boolean => {
  if (!(target instanceof Element)) return false;
  const el = target.closest(
    "button, a, [role='button'], input, select, textarea, [contenteditable=''], [contenteditable='true']",
  );
  return el !== null;
};

interface Props {
  pageTitle?: string;
  workspaces?: Array<{ id: string; name: string }>;
  selectedWorkspaceId?: string;
  onWorkspaceSelect?: (workspaceId: string) => void;
}

export const HomeHeader = ({ pageTitle }: Props) => {
  const { t } = useTranslation();
  const [platform, setPlatform] = useState<string>("macos");

  useEffect(() => {
    setPlatform(getCurrentOS());
  }, []);

  const isWindows = platform === "windows";

  const holdTimeoutRef = useRef<number | null>(null);
  const dragStartRef = useRef<{ x: number; y: number } | null>(null);
  const dragStartedRef = useRef(false);
  const activePointerIdRef = useRef<number | null>(null);
  const dragRootRef = useRef<HTMLDivElement | null>(null);

  const clearHold = useCallback(() => {
    if (holdTimeoutRef.current !== null) {
      window.clearTimeout(holdTimeoutRef.current);
      holdTimeoutRef.current = null;
    }
  }, []);

  const beginDrag = useCallback(() => {
    if (dragStartedRef.current) return;
    dragStartedRef.current = true;
    clearHold();
    void getCurrentWindow().startDragging();
  }, [clearHold]);

  useEffect(() => {
    return () => {
      clearHold();
    };
  }, [clearHold]);

  const handlePointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) return;
      if (isInteractiveTarget(e.target)) return;

      dragStartedRef.current = false;
      dragStartRef.current = { x: e.clientX, y: e.clientY };
      activePointerIdRef.current = e.pointerId;

      clearHold();
      holdTimeoutRef.current = window.setTimeout(() => {
        holdTimeoutRef.current = null;
        beginDrag();
      }, HOLD_MS);
    },
    [beginDrag, clearHold],
  );

  const handlePointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (
        dragStartedRef.current ||
        dragStartRef.current === null ||
        activePointerIdRef.current !== e.pointerId
      ) {
        return;
      }
      const dx = e.clientX - dragStartRef.current.x;
      const dy = e.clientY - dragStartRef.current.y;
      if (Math.hypot(dx, dy) > DRAG_THRESHOLD_PX) {
        beginDrag();
      }
    },
    [beginDrag],
  );

  const handlePointerEnd = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (activePointerIdRef.current !== e.pointerId) return;
      clearHold();
      dragStartRef.current = null;
      activePointerIdRef.current = null;
      dragStartedRef.current = false;
    },
    [clearHold],
  );

  const handleDoubleClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (isInteractiveTarget(e.target)) return;
      clearHold();
      void getCurrentWindow().toggleMaximize();
    },
    [clearHold],
  );

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: titlebar drag surface
    <div
      ref={dragRootRef}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerEnd}
      onPointerCancel={handlePointerEnd}
      onDoubleClick={handleDoubleClick}
      className={cn(
        "flex h-11 shrink-0 items-center justify-between border-b border-border/60 bg-card/40 pl-4 select-none",
        isWindows ? "pr-[144px]" : "pr-4",
      )}
    >
      {/* Breadcrumb Navigation */}
      <div className="flex items-center gap-1.5 text-xs">
        <span className="text-muted-foreground/70 font-medium select-none">
          BugLogin
        </span>
        <span className="text-muted-foreground/40 select-none">/</span>
        <span className="font-semibold text-foreground tracking-tight select-none">
          {pageTitle || t("rail.profiles", { defaultValue: "Profiles" })}
        </span>
      </div>

      {/* Central drag area */}
      <div className="flex-1 h-full cursor-default" />
    </div>
  );
};
export default HomeHeader;

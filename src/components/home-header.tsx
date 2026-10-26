"use client";

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuBuilding2,
  LuChevronDown,
  LuDownload,
  LuFolderPlus,
  LuGlobe,
  LuPlus,
  LuRefreshCw,
  LuUserPlus,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { getCurrentOS } from "@/lib/browser-utils";
import { cn } from "@/lib/utils";
import type { AppUpdateInfo, AppUpdateProgress } from "@/types";
import { type WorkspaceItem, WorkspaceSwitcher } from "./workspace-switcher";

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
  workspaces?: WorkspaceItem[];
  selectedWorkspaceId?: string;
  onWorkspaceSelect?: (workspaceId: string) => void;
  onManageWorkspaces?: () => void;
  onOpenOrgCenter?: () => void;
  activeWorkspace?: WorkspaceItem | null;
  onCreateProfile?: () => void;
  onCreateGroup?: () => void;
  onAddProxy?: () => void;
  onInviteMember?: () => void;
  // App update props (DES-04 §3.1)
  updateInfo?: AppUpdateInfo | null;
  updateReady?: boolean;
  isUpdating?: boolean;
  isCheckingForUpdates?: boolean;
  updateProgress?: AppUpdateProgress | null;
  onOpenUpdateDialog?: () => void;
}

export const HomeHeader = ({
  pageTitle,
  workspaces = [],
  selectedWorkspaceId,
  onWorkspaceSelect,
  onManageWorkspaces,
  onOpenOrgCenter,
  activeWorkspace,
  onCreateProfile,
  onCreateGroup,
  onAddProxy,
  onInviteMember,
  updateInfo,
  updateReady,
  isUpdating,
  isCheckingForUpdates,
  updateProgress,
  onOpenUpdateDialog,
}: Props) => {
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
      {/* Left side: Breadcrumb Navigation + Unified Context Anchor & Action Hub (DES-02) */}
      <div className="flex items-center gap-2 text-xs">
        <div className="flex items-center gap-1.5">
          <span className="text-muted-foreground/70 font-medium select-none">
            BugLogin
          </span>
          <span className="text-muted-foreground/40 select-none">/</span>
          <span className="font-semibold text-foreground tracking-tight select-none">
            {pageTitle || t("rail.profiles", { defaultValue: "Profiles" })}
          </span>
        </div>

        {/* Unified Context Anchor (DES-02 §2.1) */}
        {activeWorkspace && (
          <div className="flex items-center gap-2 ml-2 pl-2 border-l border-border/60">
            <WorkspaceSwitcher
              variant="header"
              workspaces={workspaces}
              selectedWorkspaceId={selectedWorkspaceId || ""}
              onWorkspaceSelect={onWorkspaceSelect || (() => {})}
              onManageWorkspaces={onManageWorkspaces}
              onOpenOrgCenter={onOpenOrgCenter}
            />

            {/* Unified Creation Action Hub (DES-02 §2.1) */}
            {onCreateProfile && (
              <div className="flex items-center">
                <Button
                  size="sm"
                  variant="default"
                  onClick={onCreateProfile}
                  className="h-7 px-2.5 rounded-r-none text-xs font-semibold gap-1 bg-primary text-primary-foreground hover:bg-primary/90 shadow-2xs cursor-pointer"
                  title="Tạo Profile mới (Ctrl+N)"
                >
                  <LuPlus className="size-3.5" />
                  <span>{t("actions.new", { defaultValue: "Mới" })}</span>
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      size="sm"
                      variant="default"
                      className="h-7 px-1.5 rounded-l-none border-l border-primary-foreground/20 bg-primary text-primary-foreground hover:bg-primary/90 shadow-2xs cursor-pointer"
                      aria-label="Tùy chọn tạo mới"
                    >
                      <LuChevronDown className="size-3" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="start" className="w-56">
                    <DropdownMenuLabel className="text-xs font-semibold text-muted-foreground">
                      {t("actions.addOptions", {
                        defaultValue: "Thao tác tạo nhanh",
                      })}
                    </DropdownMenuLabel>
                    <DropdownMenuItem
                      onClick={onCreateProfile}
                      className="gap-2 cursor-pointer text-xs"
                    >
                      <LuPlus className="size-4 text-primary" />
                      <div className="flex-1">
                        <div className="font-medium">
                          {t("actions.newProfile", {
                            defaultValue: "Tạo Profile mới",
                          })}
                        </div>
                      </div>
                      <DropdownMenuShortcut>Ctrl+N</DropdownMenuShortcut>
                    </DropdownMenuItem>

                    {onCreateGroup && (
                      <DropdownMenuItem
                        onClick={onCreateGroup}
                        className="gap-2 cursor-pointer text-xs"
                      >
                        <LuFolderPlus className="size-4 text-emerald-500" />
                        <div className="flex-1">
                          <div className="font-medium">
                            {t("actions.newGroup", {
                              defaultValue: "Tạo Nhóm mới",
                            })}
                          </div>
                        </div>
                        <DropdownMenuShortcut>Ctrl+G</DropdownMenuShortcut>
                      </DropdownMenuItem>
                    )}

                    <DropdownMenuSeparator />
                    <DropdownMenuLabel className="text-[11px] font-semibold text-muted-foreground/80">
                      {t("actions.resources", {
                        defaultValue: "Tài nguyên & Thành viên",
                      })}
                    </DropdownMenuLabel>

                    {onAddProxy && (
                      <DropdownMenuItem
                        onClick={onAddProxy}
                        className="gap-2 cursor-pointer text-xs"
                      >
                        <LuGlobe className="size-4 text-blue-500" />
                        <div className="flex-1">
                          <div className="font-medium">
                            {t("actions.addProxy", {
                              defaultValue: "Thêm Proxy / VPN",
                            })}
                          </div>
                        </div>
                      </DropdownMenuItem>
                    )}

                    {onInviteMember && (
                      <DropdownMenuItem
                        onClick={onInviteMember}
                        className="gap-2 cursor-pointer text-xs"
                      >
                        <LuUserPlus className="size-4 text-purple-500" />
                        <div className="flex-1">
                          <div className="font-medium">
                            {t("actions.inviteMember", {
                              defaultValue: "Mời thành viên",
                            })}
                          </div>
                        </div>
                      </DropdownMenuItem>
                    )}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Central drag area */}
      <div className="flex-1 h-full cursor-default" />

      {/* Right side: Update Anchor Pill (DES-04 §3.1) */}
      <div className="flex items-center gap-2 shrink-0">
        {updateReady && updateInfo && (
          <button
            type="button"
            onClick={onOpenUpdateDialog}
            className="flex h-6.5 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 hover:bg-emerald-500/25 transition-all shadow-2xs cursor-pointer select-none"
            title={t("appUpdate.updateReadyTooltip", {
              defaultValue: `Bản cập nhật v${updateInfo.new_version} đã sẵn sàng cài đặt`,
            })}
          >
            <span className="size-1.5 rounded-full bg-emerald-400 animate-pulse" />
            <span className="font-semibold">v{updateInfo.new_version}</span>
          </button>
        )}

        {isUpdating && updateInfo && (
          <button
            type="button"
            onClick={onOpenUpdateDialog}
            className="flex h-6.5 items-center gap-1.5 rounded-full px-2.5 text-[11px] font-mono bg-primary/10 text-primary border border-primary/25 hover:bg-primary/20 transition-all shadow-2xs cursor-pointer select-none"
            title="Đang tải bản cập nhật ngầm"
          >
            <LuDownload className="size-3 animate-bounce" />
            <span>{updateProgress?.percentage ?? 0}%</span>
          </button>
        )}

        {isCheckingForUpdates && (
          <span
            className="flex items-center gap-1 text-[11px] text-muted-foreground/60 pr-1 select-none"
            title={t("about.checkingUpdates", {
              defaultValue: "Đang kiểm tra cập nhật...",
            })}
          >
            <LuRefreshCw className="size-3 animate-spin" />
          </span>
        )}
      </div>
    </div>
  );
};

export default HomeHeader;

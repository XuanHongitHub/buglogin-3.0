"use client";

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FiWifi } from "react-icons/fi";
import { GoGear, GoKebabHorizontal } from "react-icons/go";
import {
  LuBot,
  LuCloud,
  LuCookie,
  LuFileText,
  LuFolderKanban,
  LuInfo,
  LuKeyboard,
  LuLightbulb,
  LuMessageSquareHeart,
  LuPanelLeftClose,
  LuPanelLeftOpen,
  LuPlug,
  LuPuzzle,
  LuServer,
  LuTrash2,
  LuUser,
  LuUsers,
} from "react-icons/lu";
import { useInputModality } from "@/hooks/use-input-modality";
import { getCurrentOS } from "@/lib/browser-utils";
import { launchDonutClone } from "@/lib/donut-physics";
import { MOTION_SPRING_POSITION } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { Logo } from "./icons/logo";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { type WorkspaceItem, WorkspaceSwitcher } from "./workspace-switcher";

export type AppPage =
  | "profiles"
  | "proxies"
  | "extensions"
  | "groups"
  | "members"
  | "cookieBot"
  | "agent"
  | "settings"
  | "integrations"
  | "account"
  | "shortcuts"
  | "trash"
  | "orgCenter"
  | "teamManager"
  | "auditLogs"
  | "workspaces"
  | "platformAdmin"
  | "accountManagement";

const CLICK_THRESHOLD = 5;
const CLICK_WINDOW_MS = 2000;
const LOGO_HIDDEN_KEY = "donut-logo-hidden";

function useLogoEasterEgg({
  currentPage,
  onNavigate,
}: {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
}) {
  const reduceMotion = useReducedMotion();
  const inputModality = useInputModality();
  const playfulMotion = !reduceMotion && inputModality === "pointer";
  const clickTimestamps = useRef<number[]>([]);
  const [isPressed, setIsPressed] = useState(false);
  const [wobbleKey, setWobbleKey] = useState(0);
  const [cheerKey, setCheerKey] = useState(0);
  const [isFalling, setIsFalling] = useState(false);
  const [growStep, setGrowStep] = useState(0);
  const resetTimeoutRef = useRef<number | null>(null);
  const [isHidden, setIsHidden] = useState(() => {
    try {
      return sessionStorage.getItem(LOGO_HIDDEN_KEY) === "1";
    } catch {
      return false;
    }
  });
  const logoRef = useRef<HTMLButtonElement>(null);
  const cancelFallRef = useRef<(() => void) | null>(null);

  const triggerFall = useCallback(() => {
    const el = logoRef.current;
    if (!el || isFalling) return;
    setIsFalling(true);

    cancelFallRef.current = launchDonutClone(el, {
      onExit: () => {
        try {
          sessionStorage.setItem(LOGO_HIDDEN_KEY, "1");
        } catch {
          // ignore — sessionStorage unavailable in some Tauri WebViews
        }
        setIsHidden(true);
        setIsFalling(false);
      },
    });
  }, [isFalling]);

  useEffect(() => {
    return () => {
      cancelFallRef.current?.();
    };
  }, []);

  const handleClick = useCallback(() => {
    if (isFalling || isHidden) return;

    if (currentPage !== "profiles") {
      onNavigate("profiles");
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
      return;
    }

    if (!playfulMotion) return;

    const now = Date.now();
    clickTimestamps.current = clickTimestamps.current.filter(
      (t) => now - t < CLICK_WINDOW_MS,
    );
    clickTimestamps.current.push(now);

    if (clickTimestamps.current.length >= CLICK_THRESHOLD) {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
      triggerFall();
    } else {
      setGrowStep(
        Math.min(clickTimestamps.current.length, CLICK_THRESHOLD - 1),
      );
      setWobbleKey((k) => k + 1);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
      resetTimeoutRef.current = window.setTimeout(() => {
        clickTimestamps.current = [];
        setGrowStep(0);
        resetTimeoutRef.current = null;
      }, CLICK_WINDOW_MS);
    }
  }, [
    currentPage,
    isFalling,
    isHidden,
    onNavigate,
    triggerFall,
    playfulMotion,
  ]);

  useEffect(() => {
    if (currentPage !== "profiles" || !playfulMotion) {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
    }
  }, [currentPage, playfulMotion]);

  useEffect(() => {
    if (reduceMotion) return;
    const onCheatCode = () => setCheerKey((k) => k + 1);
    window.addEventListener("donut-cheat-code", onCheatCode);
    return () => {
      window.removeEventListener("donut-cheat-code", onCheatCode);
    };
  }, [reduceMotion]);

  useEffect(() => {
    if (!reduceMotion) return;
    cancelFallRef.current?.();
    cancelFallRef.current = null;
    setIsFalling(false);
    setIsPressed(false);
    if (logoRef.current) logoRef.current.style.visibility = "";
  }, [reduceMotion]);

  useEffect(() => {
    return () => {
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
    };
  }, []);

  return {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    cheerKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
    playfulMotion,
  };
}

export interface RailNavProps {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
  onOpenAbout: () => void;
  onOpenTips?: () => void;
  onOpenFeedback?: () => void;
  onOpenAuditLogs?: () => void;
  onOpenTeamManager?: () => void;
  onOpenPlatformAdmin?: () => void;
  onOpenOrgCenter?: () => void;
  cookieBotRunning?: boolean;
  agentRequests?: number;
  workspaces?: WorkspaceItem[];
  selectedWorkspaceId?: string;
  onWorkspaceSelect?: (workspaceId: string) => void;
  onManageWorkspaces?: () => void;
  workspaceLoading?: boolean;
  totalProfilesCount?: number;
}

function ActiveIndicator() {
  const reduceMotion = useReducedMotion();
  const inputModality = useInputModality();
  const animate = !reduceMotion && inputModality === "pointer";
  return (
    <motion.span
      aria-hidden="true"
      initial={false}
      layoutId={animate ? "rail-indicator" : undefined}
      transition={animate ? MOTION_SPRING_POSITION : { duration: 0 }}
      className="absolute inset-y-1.5 left-0 w-[3px] rounded-full bg-primary"
    />
  );
}

interface RailItem {
  page: AppPage;
  Icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
}

const TOP_ITEMS: RailItem[] = [
  { page: "profiles", Icon: LuUser, labelKey: "rail.profiles" },
  { page: "groups", Icon: LuFolderKanban, labelKey: "rail.groups" },
  { page: "members", Icon: LuUsers, labelKey: "rail.members" },
  { page: "proxies", Icon: FiWifi, labelKey: "rail.network" },
  { page: "extensions", Icon: LuPuzzle, labelKey: "rail.extensions" },
  { page: "cookieBot", Icon: LuCookie, labelKey: "rail.cookieBot" },
  { page: "agent", Icon: LuBot, labelKey: "rail.agent" },
  { page: "integrations", Icon: LuPlug, labelKey: "rail.integrations" },
  { page: "account", Icon: LuCloud, labelKey: "rail.account" },
];

interface MoreMenuItem {
  page: AppPage;
  Icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
  hintKey: string;
}

const MORE_ITEMS: MoreMenuItem[] = [
  {
    page: "accountManagement",
    Icon: LuUsers,
    labelKey: "rail.more.accountManagement",
    hintKey: "rail.more.accountManagementHint",
  },
  {
    page: "shortcuts",
    Icon: LuKeyboard,
    labelKey: "rail.more.keyboardShortcuts",
    hintKey: "rail.more.keyboardShortcutsHint",
  },
  {
    page: "trash",
    Icon: LuTrash2,
    labelKey: "rail.more.trash",
    hintKey: "rail.more.trashHint",
  },
];

const isInteractiveTarget = (target: EventTarget | null): boolean => {
  if (!(target instanceof Element)) return false;
  return (
    target.closest(
      "button, a, [role='button'], input, select, textarea, [role='combobox']",
    ) !== null
  );
};

export function RailNav({
  currentPage,
  onNavigate,
  onOpenAbout,
  onOpenTips,
  onOpenFeedback,
  onOpenAuditLogs,
  onOpenTeamManager,
  onOpenPlatformAdmin,
  onOpenOrgCenter,
  cookieBotRunning = false,
  agentRequests = 0,
  workspaces = [],
  selectedWorkspaceId = "",
  onWorkspaceSelect,
  onManageWorkspaces,
  workspaceLoading = false,
  totalProfilesCount,
}: RailNavProps) {
  const { t } = useTranslation();
  const [moreOpen, setMoreOpen] = useState(false);
  const [platform, setPlatform] = useState<string>("windows");

  useEffect(() => {
    setPlatform(getCurrentOS());
  }, []);

  const isMacOS = platform === "macos";

  const [isCollapsed, setIsCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem("sidebar-collapsed") === "true";
    } catch {
      return false;
    }
  });

  const [appVersion, setAppVersion] = useState<string>("0.34.0");

  useEffect(() => {
    invoke<{ app_version: string }>("get_system_info")
      .then((info) => {
        if (info?.app_version) setAppVersion(info.app_version);
      })
      .catch(() => {});
  }, []);

  const toggleCollapse = useCallback(() => {
    setIsCollapsed((prev) => {
      const next = !prev;
      try {
        localStorage.setItem("sidebar-collapsed", String(next));
      } catch {}
      return next;
    });
  }, []);

  const {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    cheerKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
    playfulMotion,
  } = useLogoEasterEgg({ currentPage, onNavigate });

  useEffect(() => {
    if (!moreOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setMoreOpen(false);
      }
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [moreOpen]);

  const handleHeaderPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button === 0 && !isInteractiveTarget(e.target)) {
      void getCurrentWindow().startDragging();
    }
  };

  return (
    <nav
      className={cn(
        "relative flex h-full shrink-0 flex-col border-r border-border bg-card/70 backdrop-blur-xs select-none transition-[width] duration-200 ease-in-out",
        isCollapsed ? "w-14 items-center" : "w-60",
      )}
    >
      {/* macOS traffic light spacer if native titlebar is transparent/overlay */}
      {isMacOS && (
        <div
          aria-hidden="true"
          className="flex h-7 shrink-0 items-center gap-[7px] px-3 pt-2"
        >
          <div className="size-[11px] rounded-full" />
          <div className="size-[11px] rounded-full" />
          <div className="size-[11px] rounded-full" />
        </div>
      )}

      {/* Top Header / App Brand */}
      <div
        onPointerDown={handleHeaderPointerDown}
        className={cn(
          "flex h-11 shrink-0 items-center border-b border-border/40 px-2.5",
          isCollapsed ? "justify-center" : "justify-between gap-2",
        )}
      >
        <div className="flex items-center gap-2 min-w-0">
          {!isHidden ? (
            <button
              ref={logoRef}
              type="button"
              aria-label="BugLogin"
              className="grid size-7 shrink-0 cursor-pointer place-items-center rounded-md bg-transparent text-foreground select-none hover:bg-muted/50 transition-colors"
              onClick={handleClick}
              onPointerDown={() => {
                setIsPressed(true);
              }}
              onPointerUp={() => {
                setIsPressed(false);
              }}
              onPointerLeave={() => {
                setIsPressed(false);
              }}
            >
              <span
                style={{
                  transform: !playfulMotion
                    ? "none"
                    : isPressed
                      ? `scale(${(1 + growStep * 0.25) * 0.9})`
                      : `scale(${1 + growStep * 0.25})`,
                }}
                className="inline-grid place-items-center transition-transform duration-300 ease-out motion-reduce:transition-none"
              >
                <span
                  key={`${wobbleKey}:${cheerKey}`}
                  className={cn(
                    "inline-grid place-items-center",
                    !isFalling &&
                      !isPressed &&
                      ((playfulMotion && wobbleKey > 0) || cheerKey > 0) &&
                      "animate-[wiggle_0.3s_ease-in-out]",
                  )}
                >
                  <Logo className="size-5 will-change-transform" />
                </span>
              </span>
            </button>
          ) : (
            <div className="size-7 shrink-0" />
          )}

          {!isCollapsed && (
            <div className="flex items-baseline gap-1.5 min-w-0">
              <span className="truncate text-xs font-bold tracking-tight text-foreground select-none">
                BugLogin
              </span>
              <span className="text-[10px] font-mono font-medium text-muted-foreground/60 select-none">
                v{appVersion}
              </span>
            </div>
          )}
        </div>

        {!isCollapsed && (
          <button
            type="button"
            onClick={toggleCollapse}
            aria-label="Collapse sidebar"
            title="Collapse sidebar"
            className="grid size-6 shrink-0 cursor-pointer place-items-center rounded text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          >
            <LuPanelLeftClose className="size-3.5" />
          </button>
        )}
      </div>

      {onWorkspaceSelect && (
        <div
          className={cn(
            "shrink-0 border-b border-border/40 py-2",
            isCollapsed ? "flex justify-center px-2" : "px-2.5",
          )}
        >
          <WorkspaceSwitcher
            workspaces={workspaces}
            selectedWorkspaceId={selectedWorkspaceId}
            onWorkspaceSelect={onWorkspaceSelect}
            onManageWorkspaces={onManageWorkspaces}
            isLoading={workspaceLoading}
            onOpenOrgCenter={onOpenOrgCenter ?? (() => onNavigate("orgCenter"))}
            collapsed={isCollapsed}
            totalProfilesCount={totalProfilesCount}
          />
        </div>
      )}

      {/* Primary Navigation Items */}
      <div
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto py-2.5 [-ms-overflow-style:none] [&::-webkit-scrollbar]:hidden",
          isCollapsed ? "items-center px-2" : "px-2",
        )}
      >
        {TOP_ITEMS.map(({ page, Icon, labelKey }) => {
          const active = currentPage === page;
          const waiting = page === "agent" ? agentRequests : 0;
          const label =
            waiting > 0
              ? t("rail.agentWaiting", { count: waiting })
              : page === "cookieBot" && cookieBotRunning
                ? t("rail.cookieBotRunning")
                : t(labelKey);

          if (isCollapsed) {
            return (
              <Tooltip key={page} delayDuration={200}>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => onNavigate(page)}
                    aria-label={label}
                    aria-current={active ? "page" : undefined}
                    className={cn(
                      "relative grid size-8 shrink-0 cursor-pointer place-items-center rounded-lg transition-colors duration-100",
                      active
                        ? "bg-accent text-accent-foreground font-semibold shadow-xs"
                        : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
                    )}
                  >
                    {active && <ActiveIndicator />}
                    <Icon className="size-4" />
                    {page === "cookieBot" && cookieBotRunning && (
                      <span
                        aria-hidden="true"
                        className="absolute top-1 right-1 size-1.5 rounded-full bg-success"
                      />
                    )}
                    {waiting > 0 && (
                      <span
                        aria-hidden="true"
                        data-testid="agent-rail-badge"
                        className="absolute -top-1 -right-1 grid h-3.5 min-w-3.5 place-items-center rounded-full bg-warning px-1 text-[9px] leading-none font-semibold tabular-nums text-warning-foreground"
                      >
                        {waiting > 9 ? "9+" : waiting}
                      </span>
                    )}
                  </button>
                </TooltipTrigger>
                <TooltipContent side="right">
                  <div className="flex items-center gap-1.5 text-xs">
                    <span>{label}</span>
                    {page === "profiles" &&
                      totalProfilesCount !== undefined && (
                        <span className="text-muted-foreground">
                          ({totalProfilesCount})
                        </span>
                      )}
                  </div>
                </TooltipContent>
              </Tooltip>
            );
          }

          return (
            <button
              key={page}
              type="button"
              onClick={() => onNavigate(page)}
              aria-label={label}
              aria-current={active ? "page" : undefined}
              className={cn(
                "group relative flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors duration-100 select-none",
                active
                  ? "bg-accent text-accent-foreground font-semibold shadow-xs"
                  : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
              )}
            >
              {active && <ActiveIndicator />}
              <Icon
                className={cn(
                  "size-4 shrink-0 transition-colors",
                  active
                    ? "text-accent-foreground"
                    : "text-muted-foreground group-hover:text-accent-foreground",
                )}
              />
              <span className="truncate flex-1 text-left">{label}</span>

              {page === "profiles" && totalProfilesCount !== undefined && (
                <span
                  className={cn(
                    "text-[10px] tabular-nums font-semibold px-1.5 py-0.5 rounded-full shrink-0 transition-colors",
                    active
                      ? "bg-primary/20 text-primary"
                      : "bg-muted text-muted-foreground group-hover:bg-muted/80",
                  )}
                >
                  {totalProfilesCount}
                </span>
              )}

              {page === "cookieBot" && cookieBotRunning && (
                <span
                  aria-hidden="true"
                  className="size-2 rounded-full bg-success animate-pulse shrink-0"
                />
              )}

              {waiting > 0 && (
                <span
                  aria-hidden="true"
                  data-testid="agent-rail-badge"
                  className="grid h-3.5 min-w-3.5 place-items-center rounded-full bg-warning px-1 text-[9px] leading-none font-semibold tabular-nums text-warning-foreground shrink-0"
                >
                  {waiting > 9 ? "9+" : waiting}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {/* Bottom Section: More, Settings, Expand/Collapse */}
      <div
        className={cn(
          "shrink-0 border-t border-border/40 py-2 flex flex-col gap-1",
          isCollapsed ? "items-center px-2" : "px-2",
        )}
      >
        {/* More Menu */}
        {isCollapsed ? (
          <Tooltip delayDuration={200}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => setMoreOpen((v) => !v)}
                aria-label={t("rail.more.label")}
                aria-expanded={moreOpen}
                className={cn(
                  "grid size-8 shrink-0 cursor-pointer place-items-center rounded-lg transition-colors duration-100",
                  moreOpen
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
                )}
              >
                <GoKebabHorizontal className="size-4" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{t("rail.more.label")}</TooltipContent>
          </Tooltip>
        ) : (
          <button
            type="button"
            onClick={() => setMoreOpen((v) => !v)}
            aria-label={t("rail.more.label")}
            aria-expanded={moreOpen}
            className={cn(
              "flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors duration-100 select-none",
              moreOpen
                ? "bg-accent text-accent-foreground"
                : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
            )}
          >
            <GoKebabHorizontal className="size-4 shrink-0 text-muted-foreground" />
            <span className="truncate flex-1 text-left">
              {t("rail.more.label")}
            </span>
          </button>
        )}

        {/* Settings Button */}
        {isCollapsed ? (
          <Tooltip delayDuration={200}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => onNavigate("settings")}
                aria-label={t("rail.settings")}
                aria-current={currentPage === "settings" ? "page" : undefined}
                className={cn(
                  "relative grid size-8 shrink-0 cursor-pointer place-items-center rounded-lg transition-colors duration-100",
                  currentPage === "settings"
                    ? "bg-accent text-accent-foreground font-semibold shadow-xs"
                    : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
                )}
              >
                {currentPage === "settings" && <ActiveIndicator />}
                <GoGear className="size-4" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{t("rail.settings")}</TooltipContent>
          </Tooltip>
        ) : (
          <button
            type="button"
            onClick={() => onNavigate("settings")}
            aria-label={t("rail.settings")}
            aria-current={currentPage === "settings" ? "page" : undefined}
            className={cn(
              "group relative flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors duration-100 select-none",
              currentPage === "settings"
                ? "bg-accent text-accent-foreground font-semibold shadow-xs"
                : "text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground",
            )}
          >
            {currentPage === "settings" && <ActiveIndicator />}
            <GoGear
              className={cn(
                "size-4 shrink-0 transition-colors",
                currentPage === "settings"
                  ? "text-accent-foreground"
                  : "text-muted-foreground group-hover:text-accent-foreground",
              )}
            />
            <span className="truncate flex-1 text-left">
              {t("rail.settings")}
            </span>
          </button>
        )}

        {/* Bottom Expand Toggle when collapsed */}
        {isCollapsed && (
          <Tooltip delayDuration={200}>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={toggleCollapse}
                aria-label="Expand sidebar"
                className="grid size-8 shrink-0 cursor-pointer place-items-center rounded-lg text-muted-foreground hover:bg-accent/60 hover:text-accent-foreground transition-colors"
              >
                <LuPanelLeftOpen className="size-4" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">Expand sidebar</TooltipContent>
          </Tooltip>
        )}
      </div>

      {/* Floating More Menu */}
      {moreOpen && (
        <>
          <button
            type="button"
            aria-label={t("rail.more.closeAriaLabel")}
            className="fixed inset-0 z-30 cursor-default bg-transparent"
            onClick={() => setMoreOpen(false)}
          />
          <div
            role="menu"
            aria-label={t("rail.more.label")}
            className={cn(
              "absolute z-40 w-56 rounded-xl border border-border bg-card p-1 text-card-foreground shadow-xl",
              isCollapsed ? "bottom-12 left-14" : "bottom-12 left-3",
            )}
          >
            {MORE_ITEMS.map(({ page, Icon, labelKey, hintKey }) => (
              <button
                key={page}
                type="button"
                role="menuitem"
                onClick={() => {
                  setMoreOpen(false);
                  onNavigate(page);
                }}
                className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
              >
                <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                  <Icon className="size-3.5" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-xs font-medium text-foreground">
                    {t(labelKey)}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t(hintKey)}
                  </span>
                </span>
              </button>
            ))}

            {onOpenAuditLogs && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMoreOpen(false);
                  onOpenAuditLogs();
                }}
                className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
              >
                <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                  <LuFileText className="size-3.5 text-primary" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-xs font-medium text-foreground">
                    {t("rail.more.auditLogs", {
                      defaultValue: "Nhật Ký Hoạt Động & Kiểm Toán",
                    })}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t("rail.more.auditLogsHint", {
                      defaultValue: "Lịch sử hoạt động và bảo mật hệ thống",
                    })}
                  </span>
                </span>
              </button>
            )}

            {onOpenTeamManager && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMoreOpen(false);
                  onOpenTeamManager();
                }}
                className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
              >
                <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                  <LuUsers className="size-3.5 text-primary" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-xs font-medium text-foreground">
                    {t("rail.more.teamManager", {
                      defaultValue: "Điều Hành Đội Ngũ",
                    })}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t("rail.more.teamManagerHint", {
                      defaultValue: "Quản lý nhân viên và phân bổ hồ sơ",
                    })}
                  </span>
                </span>
              </button>
            )}

            {onOpenPlatformAdmin && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMoreOpen(false);
                  onOpenPlatformAdmin();
                }}
                className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
              >
                <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                  <LuServer className="size-3.5 text-primary" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-xs font-medium text-foreground">
                    {t("rail.more.platformAdmin", {
                      defaultValue: "Quản Trị Nền Tảng",
                    })}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t("rail.more.platformAdminHint", {
                      defaultValue: "Quản lý tổ chức và hạn mức hệ thống",
                    })}
                  </span>
                </span>
              </button>
            )}

            <button
              type="button"
              role="menuitem"
              data-slot="rail-open-tips"
              onClick={() => {
                setMoreOpen(false);
                onOpenTips?.();
              }}
              className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
            >
              <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                <LuLightbulb className="size-3.5" />
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-xs font-medium text-foreground">
                  {t("rail.more.tips")}
                </span>
                <span className="truncate text-[10px] text-muted-foreground">
                  {t("rail.more.tipsHint")}
                </span>
              </span>
            </button>

            <button
              type="button"
              role="menuitem"
              data-slot="rail-open-feedback"
              onClick={() => {
                setMoreOpen(false);
                onOpenFeedback?.();
              }}
              className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
            >
              <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                <LuMessageSquareHeart className="size-3.5" />
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-xs font-medium text-foreground">
                  {t("rail.more.feedback")}
                </span>
                <span className="truncate text-[10px] text-muted-foreground">
                  {t("rail.more.feedbackHint")}
                </span>
              </span>
            </button>

            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setMoreOpen(false);
                onOpenAbout();
              }}
              className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent hover:text-accent-foreground"
            >
              <span className="grid size-5 shrink-0 place-items-center text-muted-foreground">
                <LuInfo className="size-3.5" />
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-xs font-medium text-foreground">
                  {t("rail.more.about")}
                </span>
                <span className="truncate text-[10px] text-muted-foreground">
                  {t("rail.more.aboutHint")}
                </span>
              </span>
            </button>
          </div>
        </>
      )}
    </nav>
  );
}

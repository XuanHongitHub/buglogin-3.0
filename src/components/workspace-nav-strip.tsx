"use client";

import { useTranslation } from "react-i18next";
import { FiWifi } from "react-icons/fi";
import { GoGear } from "react-icons/go";
import {
  LuFileText,
  LuFolderKanban,
  LuLayers,
  LuUser,
  LuUsers,
} from "react-icons/lu";
import type { AppPage } from "@/components/rail-nav";
import { cn } from "@/lib/utils";

export interface WorkspaceNavStripProps {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
  profilesCount?: number;
  groupsCount?: number;
  membersCount?: number;
  proxiesCount?: number;
  workspaceName?: string;
  workspaceRole?: string;
  organizationName?: string;
  maxProfiles?: number;
  className?: string;
}

export function WorkspaceNavStrip({
  currentPage,
  onNavigate,
  profilesCount = 0,
  groupsCount = 0,
  membersCount,
  proxiesCount = 0,
  workspaceName,
  workspaceRole,
  organizationName,
  maxProfiles,
  className,
}: WorkspaceNavStripProps) {
  const { t } = useTranslation();

  const tabs: Array<{
    id: AppPage;
    label: string;
    Icon: React.ComponentType<{ className?: string }>;
    count?: number;
  }> = [
    {
      id: "profiles",
      label: t("rail.profiles", { defaultValue: "Profiles" }),
      Icon: LuUser,
      count: profilesCount,
    },
    {
      id: "groups",
      label: t("rail.groups", { defaultValue: "Nhóm" }),
      Icon: LuFolderKanban,
      count: groupsCount,
    },
    {
      id: "members",
      label: t("rail.members", { defaultValue: "Thành viên" }),
      Icon: LuUsers,
      count: membersCount,
    },
    {
      id: "proxies",
      label: t("rail.network", { defaultValue: "Proxy / VPN" }),
      Icon: FiWifi,
      count: proxiesCount,
    },
    {
      id: "auditLogs",
      label: t("rail.auditLogs", { defaultValue: "Audit Log" }),
      Icon: LuFileText,
    },
    {
      id: "settings",
      label: t("rail.settings", { defaultValue: "Cài đặt" }),
      Icon: GoGear,
    },
  ];

  const quotaPercent =
    maxProfiles && maxProfiles > 0
      ? Math.min(100, Math.round((profilesCount / maxProfiles) * 100))
      : null;

  return (
    <div
      className={cn(
        "flex h-9 shrink-0 items-center justify-between border-b border-border/60 bg-muted/20 px-3 select-none",
        className,
      )}
    >
      {/* Sub-header Navigation Tabs */}
      <nav
        aria-label="Workspace tabs"
        className="flex items-center gap-1 overflow-x-auto no-scrollbar"
      >
        {tabs.map((tab) => {
          const isActive = currentPage === tab.id;
          const Icon = tab.Icon;
          return (
            <button
              key={tab.id}
              type="button"
              onClick={() => onNavigate(tab.id)}
              className={cn(
                "group relative flex h-7 items-center gap-1.5 px-2.5 rounded-md text-xs font-medium transition-all duration-150 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-primary",
                isActive
                  ? "bg-background text-foreground shadow-2xs border border-border/70"
                  : "text-muted-foreground/80 hover:text-foreground hover:bg-muted/60",
              )}
            >
              <Icon
                className={cn(
                  "size-3.5 transition-colors",
                  isActive
                    ? "text-primary"
                    : "text-muted-foreground/70 group-hover:text-foreground",
                )}
              />
              <span>{tab.label}</span>
              {tab.count !== undefined && (
                <span
                  className={cn(
                    "ml-0.5 rounded-full px-1.5 py-0.2 text-[10px] font-mono leading-none",
                    isActive
                      ? "bg-primary/10 text-primary font-semibold"
                      : "bg-muted text-muted-foreground/70 group-hover:text-muted-foreground",
                  )}
                >
                  {tab.count}
                </span>
              )}
            </button>
          );
        })}
      </nav>

      {/* Inline Compact Status Ribbon (h-7, DES-03 §2) */}
      <div className="hidden sm:flex items-center gap-2 font-mono text-[11px] text-muted-foreground/80">
        {organizationName && (
          <span className="truncate max-w-[120px] text-foreground/80 font-medium">
            {organizationName}
          </span>
        )}
        {workspaceName && (
          <>
            <span className="text-muted-foreground/40">•</span>
            <span className="truncate max-w-[120px] font-medium text-foreground">
              {workspaceName}
            </span>
          </>
        )}
        {workspaceRole && (
          <>
            <span className="text-muted-foreground/40">•</span>
            <span className="inline-flex items-center px-1.5 py-0.2 rounded border text-[9px] font-bold uppercase tracking-wider bg-amber-500/10 text-amber-500 border-amber-500/30">
              {workspaceRole}
            </span>
          </>
        )}
        {quotaPercent !== null && (
          <>
            <span className="text-muted-foreground/40">•</span>
            <span>
              Quota: {profilesCount}/{maxProfiles}{" "}
              <span
                className={cn(
                  quotaPercent > 90
                    ? "text-destructive font-bold"
                    : "text-primary font-medium",
                )}
              >
                ({quotaPercent}%)
              </span>
            </span>
          </>
        )}
      </div>
    </div>
  );
}

export default WorkspaceNavStrip;

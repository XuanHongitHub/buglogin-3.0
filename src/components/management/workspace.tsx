"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuBuilding2,
  LuDatabase,
  LuFolderKanban,
  LuLayoutDashboard,
  LuPanelsTopLeft,
  LuUsers,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { ResourceInventoryPanel } from "../resource-inventory-panel";
import { WorkspaceMembers } from "./members";
import { ManagementResources } from "./resources";

type Tab = "overview" | "members" | "groups" | "profiles" | "resources";
export type ManagementWorkspaceTab = Tab;

export function ManagementWorkspace({
  organizationId,
  organizationName,
  workspaceId,
  workspaceName,
  initialTab = "overview",
  onBack,
  onTabChange,
  onDataChanged,
}: {
  organizationId: string;
  organizationName: string;
  workspaceId: string;
  workspaceName: string;
  initialTab?: Tab;
  onBack: () => void;
  onTabChange?: (tab: Tab) => void;
  onDataChanged?: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [tabSelection, setTabSelection] = useState<{
    workspaceId: string;
    tab: Tab;
  } | null>(null);
  const tab =
    tabSelection?.workspaceId === workspaceId
      ? tabSelection.tab
      : initialTab !== "overview"
        ? initialTab
        : readWorkspaceTab(workspaceId);
  useEffect(() => {
    setTabSelection({
      workspaceId,
      tab:
        initialTab !== "overview" ? initialTab : readWorkspaceTab(workspaceId),
    });
  }, [initialTab, workspaceId]);
  const setTab = (next: Tab) => {
    setTabSelection({ workspaceId, tab: next });
    onTabChange?.(next);
    try {
      window.sessionStorage.setItem(workspaceTabKey(workspaceId), next);
    } catch {
      // Tab navigation remains available if desktop storage is unavailable.
    }
  };
  const tabs: { id: Tab; label: string; icon: React.ReactNode }[] = [
    { id: "overview", label: t("overview"), icon: <LuLayoutDashboard /> },
    { id: "members", label: t("members"), icon: <LuUsers /> },
    { id: "groups", label: t("groups"), icon: <LuFolderKanban /> },
    { id: "profiles", label: t("profiles"), icon: <LuPanelsTopLeft /> },
    { id: "resources", label: t("resources"), icon: <LuDatabase /> },
  ];
  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <header className="flex shrink-0 items-center gap-3 border-b border-border px-4 py-3">
        <Button
          variant="ghost"
          size="icon"
          className="size-8 shrink-0"
          onClick={onBack}
          aria-label={t("back")}
        >
          <LuArrowLeft />
        </Button>
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs text-muted-foreground">
            {organizationName} / {workspaceName}
          </p>
          <h1 className="truncate text-base font-semibold">{workspaceName}</h1>
        </div>
        <nav
          className="flex shrink-0 items-center gap-1"
          aria-label={`${organizationName} / ${workspaceName}`}
        >
          {tabs.map((item) => (
            <Button
              key={item.id}
              size="sm"
              variant={tab === item.id ? "secondary" : "ghost"}
              onClick={() => setTab(item.id)}
              aria-pressed={tab === item.id}
            >
              {item.icon}
              <span className="hidden sm:inline">{item.label}</span>
            </Button>
          ))}
        </nav>
      </header>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {tab === "overview" ? (
          <div className="grid content-start gap-3 p-4 md:grid-cols-2">
            <button
              type="button"
              onClick={() => setTab("members")}
              className="flex items-start gap-3 rounded-md border border-border p-4 text-left transition-colors hover:bg-muted/40"
            >
              <LuUsers className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <span>
                <span className="block text-sm font-medium">
                  {t("members")}
                </span>
                <span className="mt-1 block text-xs text-muted-foreground">
                  {t("memberIntro", {
                    defaultValue:
                      "Manage workspace roles, invitations and group access.",
                  })}
                </span>
              </span>
            </button>
            <button
              type="button"
              onClick={() => setTab("groups")}
              className="flex items-start gap-3 rounded-md border border-border p-4 text-left transition-colors hover:bg-muted/40"
            >
              <LuFolderKanban className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <span>
                <span className="block text-sm font-medium">{t("groups")}</span>
                <span className="mt-1 block text-xs text-muted-foreground">
                  {t("groupIntro", {
                    defaultValue:
                      "Create groups and manage who can access each group.",
                  })}
                </span>
              </span>
            </button>
            <button
              type="button"
              onClick={() => setTab("profiles")}
              className="flex items-start gap-3 rounded-md border border-border p-4 text-left transition-colors hover:bg-muted/40"
            >
              <LuBuilding2 className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <span>
                <span className="block text-sm font-medium">
                  {t("profiles")}
                </span>
                <span className="mt-1 block text-xs text-muted-foreground">
                  {t("profileIntro", {
                    defaultValue:
                      "Manage profile metadata and group assignment.",
                  })}
                </span>
              </span>
            </button>
            <button
              type="button"
              onClick={() => setTab("resources")}
              className="flex items-start gap-3 rounded-md border border-border p-4 text-left transition-colors hover:bg-muted/40"
            >
              <LuDatabase className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <span>
                <span className="block text-sm font-medium">
                  {t("resources")}
                </span>
                <span className="mt-1 block text-xs text-muted-foreground">
                  {t("resourceIntro", {
                    defaultValue:
                      "Review workspace resources and their direct access grants.",
                  })}
                </span>
              </span>
            </button>
          </div>
        ) : null}
        {tab === "members" ? (
          <WorkspaceMembers
            organizationId={organizationId}
            workspaceId={workspaceId}
            onDataChanged={onDataChanged}
          />
        ) : null}
        {tab === "groups" ? (
          <ManagementResources
            organizationName={organizationName}
            workspaceName={workspaceName}
            workspaceId={workspaceId}
            kind="groups"
          />
        ) : null}
        {tab === "profiles" ? (
          <ManagementResources
            organizationName={organizationName}
            workspaceName={workspaceName}
            workspaceId={workspaceId}
            kind="profiles"
          />
        ) : null}
        {tab === "resources" ? (
          <ResourceInventoryPanel
            initialOrganizationId={organizationId}
            initialWorkspaceId={workspaceId}
            initialWorkspaceName={workspaceName}
            showWorkspaceSelector={false}
          />
        ) : null}
      </div>
    </section>
  );
}

function workspaceTabKey(workspaceId: string) {
  return `buglogin:management:workspace-tab:${workspaceId}:v1`;
}

function readWorkspaceTab(workspaceId: string): Tab {
  if (typeof window === "undefined") return "overview";
  try {
    const saved = window.sessionStorage.getItem(workspaceTabKey(workspaceId));
    return saved === "members" ||
      saved === "groups" ||
      saved === "profiles" ||
      saved === "resources"
      ? saved
      : "overview";
  } catch {
    return "overview";
  }
}

"use client";

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuBuilding2,
  LuFolderKanban,
  LuLayoutDashboard,
  LuPanelsTopLeft,
  LuUsers,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { WorkspaceMembers } from "./members";
import { ManagementResources } from "./resources";

type Tab = "overview" | "members" | "groups" | "profiles";

export function ManagementWorkspace({
  organizationId,
  organizationName,
  workspaceId,
  workspaceName,
  onBack,
  onDataChanged,
}: {
  organizationId: string;
  organizationName: string;
  workspaceId: string;
  workspaceName: string;
  onBack: () => void;
  onDataChanged?: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [tab, setTab] = useState<Tab>("overview");
  const tabs: { id: Tab; label: string; icon: React.ReactNode }[] = [
    { id: "overview", label: t("overview"), icon: <LuLayoutDashboard /> },
    { id: "members", label: t("members"), icon: <LuUsers /> },
    { id: "groups", label: t("groups"), icon: <LuFolderKanban /> },
    { id: "profiles", label: t("profiles"), icon: <LuPanelsTopLeft /> },
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
          aria-label={workspaceName}
        >
          {tabs.map((item) => (
            <Button
              key={item.id}
              size="sm"
              variant={tab === item.id ? "secondary" : "ghost"}
              onClick={() => setTab(item.id)}
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
          <ManagementResources workspaceId={workspaceId} kind="groups" />
        ) : null}
        {tab === "profiles" ? (
          <ManagementResources workspaceId={workspaceId} kind="profiles" />
        ) : null}
      </div>
    </section>
  );
}

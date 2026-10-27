"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuBuilding2,
  LuClipboardList,
  LuHardDrive,
  LuUsers,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { createTauriDesktopApi } from "../../../apps/desktop/src/bridge";
import { AccountDirectory } from "./accounts";
import { OrganizationManagement } from "./org-workspaces";

type View = "organizations" | "workspaces" | "quotas" | "accounts";
const api = createTauriDesktopApi();

export function PlatformManagement({
  onOpenAudit,
}: {
  onOpenAudit: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [view, setView] = useState<View>("organizations");
  const [isPlatformAdmin, setIsPlatformAdmin] = useState<boolean | null>(null);
  useEffect(() => {
    let active = true;
    void api
      .getAccount()
      .then((account) => {
        if (active) setIsPlatformAdmin(account.platformAdmin);
      })
      .catch((error: unknown) => {
        console.warn("Failed to determine management scope:", error);
      });
    return () => {
      active = false;
    };
  }, []);
  const options: { id: View; icon: React.ReactNode; label: string }[] = [
    { id: "organizations", icon: <LuBuilding2 />, label: t("organizations") },
    { id: "workspaces", icon: <LuHardDrive />, label: t("workspaces") },
    { id: "quotas", icon: <LuHardDrive />, label: t("quotas") },
    { id: "accounts", icon: <LuUsers />, label: t("directory") },
  ];
  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <header className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2.5">
        <h1 className="text-base font-semibold">
          {isPlatformAdmin === null
            ? t("management")
            : t(isPlatformAdmin ? "platform" : "organizationManagement")}
        </h1>
        <div className="flex min-w-0 items-center gap-1 overflow-x-auto">
          {options.map((item) => (
            <Button
              key={item.id}
              size="sm"
              variant={view === item.id ? "secondary" : "ghost"}
              onClick={() => setView(item.id)}
            >
              {item.icon}
              <span>{item.label}</span>
            </Button>
          ))}
          <Button size="sm" variant="ghost" onClick={onOpenAudit}>
            <LuClipboardList />
            <span>{t("audit")}</span>
          </Button>
        </div>
      </header>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {view === "organizations" ? (
          <OrganizationManagement platformView="organizations" />
        ) : null}
        {view === "workspaces" ? (
          <OrganizationManagement platformView="workspaces" />
        ) : null}
        {view === "quotas" ? (
          <OrganizationManagement platformView="quotas" />
        ) : null}
        {view === "accounts" ? <AccountDirectory /> : null}
      </div>
    </section>
  );
}

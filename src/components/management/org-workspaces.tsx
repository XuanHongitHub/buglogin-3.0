"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuArrowRight,
  LuBuilding2,
  LuCheck,
  LuChevronDown,
  LuCircleAlert,
  LuDatabase,
  LuDownload,
  LuEllipsis,
  LuFileUp,
  LuFolderKanban,
  LuKeyRound,
  LuLoaderCircle,
  LuPanelsTopLeft,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuSettings2,
  LuTrash2,
  LuUsers,
  LuX,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { showSuccessToast } from "@/lib/toast-utils";
import { createTauriDesktopApi } from "../../../apps/desktop/src/bridge";
import type {
  AccountView,
  Organization,
  OrganizationMemberView,
  OrganizationRecord,
  Workspace,
  WorkspaceRecord,
} from "../../../frontend/src/generated/command-registry";
import { AccountDirectory, AccountProvisioning } from "./accounts";
import {
  type PendingTenancyDelete,
  parseOrganizationManagementRoute,
  parsePendingTenancyDeletes,
  removePendingTenancyDelete,
  serializeOrganizationManagementRoute,
  serializePendingTenancyDeletes,
  upsertPendingTenancyDelete,
} from "./org-management-route";
import { ManagementWorkspace, type ManagementWorkspaceTab } from "./workspace";
import "./org-platform-i18n";

const api = createTauriDesktopApi();
const PAGE_SIZE = 25;
type WorkspaceUsage = { profiles: number; members: number };
type WorkspaceEntry = {
  organization: Organization;
  workspace: Workspace;
  usage: WorkspaceUsage | null;
  usageError: string | null;
};
type OrgEntry = {
  organization: Organization;
  owner: string | null;
  workspaces: Workspace[] | null;
  error: string | null;
};
type BatchRow = {
  key: string;
  name: string;
  profiles: string;
  members: string;
  record?: WorkspaceRecord;
  usage?: WorkspaceUsage | null;
  status?: "succeeded" | "failed";
  error?: string;
};
type Route = { orgId: string | null; workspaceId: string | null };
type PlatformView = "organizations" | "workspaces" | "quotas";

type SavedManagementState = {
  route: Route;
  orgTab: "workspaces" | "accounts" | "settings";
  search: string;
  page: number;
};

function readManagementState(key: string): SavedManagementState {
  const empty: SavedManagementState = {
    route: { orgId: null, workspaceId: null },
    orgTab: "workspaces",
    search: "",
    page: 1,
  };
  if (typeof window === "undefined") return empty;
  try {
    const value = JSON.parse(window.sessionStorage.getItem(key) ?? "null");
    if (!value || typeof value !== "object") return empty;
    return {
      route: {
        orgId:
          typeof value.route?.orgId === "string" ? value.route.orgId : null,
        workspaceId:
          typeof value.route?.workspaceId === "string"
            ? value.route.workspaceId
            : null,
      },
      orgTab: ["workspaces", "accounts", "settings"].includes(value.orgTab)
        ? value.orgTab
        : empty.orgTab,
      search: typeof value.search === "string" ? value.search : "",
      page: Number.isSafeInteger(value.page) && value.page > 0 ? value.page : 1,
    };
  } catch {
    return empty;
  }
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (
    typeof error === "object" &&
    error !== null &&
    "title" in error &&
    typeof error.title === "string"
  )
    return error.title;
  return String(error);
}

function readPendingTenancyDeletes(storageKey: string): PendingTenancyDelete[] {
  if (typeof window === "undefined") return [];
  try {
    return parsePendingTenancyDeletes(
      window.sessionStorage.getItem(storageKey),
    );
  } catch {
    return [];
  }
}

function pendingTenancyDeleteFor(
  target: Organization | WorkspaceEntry,
): PendingTenancyDelete {
  if ("workspace" in target) {
    return {
      kind: "workspace",
      organizationId: target.organization.id,
      organizationName: target.organization.name,
      workspaceId: target.workspace.id,
      workspaceName: target.workspace.name,
    };
  }
  return {
    kind: "organization",
    organizationId: target.id,
    organizationName: target.name,
  };
}

function parseRows(text: string): string[][] {
  const separator = text.includes("\t") ? "\t" : ",";
  const rows: string[][] = [];
  let row: string[] = [],
    value = "",
    quoted = false;
  const source = text.replace(/^\uFEFF/, "");
  for (let i = 0; i < source.length; i++) {
    const char = source[i];
    if (char === '"') {
      if (quoted && source[i + 1] === '"') {
        value += '"';
        i++;
      } else if (quoted || value.length === 0) quoted = !quoted;
      else value += char;
    } else if (char === separator && !quoted) {
      row.push(value.trim());
      value = "";
    } else if ((char === "\n" || char === "\r") && !quoted) {
      if (char === "\r" && source[i + 1] === "\n") i++;
      row.push(value.trim());
      if (row.some(Boolean)) rows.push(row);
      row = [];
      value = "";
    } else value += char;
  }
  if (quoted) throw new Error(i18nText("invalidCsv"));
  row.push(value.trim());
  if (row.some(Boolean)) rows.push(row);
  return rows;
}
function i18nText(key: string) {
  return importI18n.t(key, { ns: "managementOrg" });
}

import importI18n from "@/i18n";

async function loadUsage(workspaceId: string): Promise<WorkspaceUsage> {
  const [members, profiles] = await Promise.all([
    api.listMembers(workspaceId),
    (async () => {
      let cursor: string | undefined;
      const seen = new Set<string>();
      let count = 0;
      do {
        const page = await api.listProfiles(false, {
          workspaceId,
          limit: 100,
          ...(cursor ? { cursor } : {}),
        });
        count += page.items.length;
        cursor = page.nextCursor ?? undefined;
        if (cursor && seen.has(cursor))
          throw new Error("Profile cursor did not advance");
        if (cursor) seen.add(cursor);
      } while (cursor);
      return count;
    })(),
  ]);
  return { profiles, members: members.items.length };
}

function EmptyState({
  text,
  action,
}: {
  text: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="grid min-h-48 place-content-center justify-items-center gap-3 px-4 text-center">
      <LuBuilding2 className="size-6 text-muted-foreground" />
      <p className="text-sm text-muted-foreground">{text}</p>
      {action}
    </div>
  );
}

function ErrorNotice({
  message,
  onRetry,
}: {
  message: string;
  onRetry?: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  return (
    <div
      role="alert"
      className="mb-3 flex items-start justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm"
    >
      <div className="flex min-w-0 items-start gap-2">
        <LuCircleAlert className="mt-0.5 size-4 shrink-0 text-destructive" />
        <span className="break-words">{message}</span>
      </div>
      {onRetry ? (
        <Button variant="outline" size="sm" onClick={onRetry}>
          {t("retry")}
        </Button>
      ) : null}
    </div>
  );
}

function Pager({
  count,
  page,
  onPage,
}: {
  count: number;
  page: number;
  onPage: (page: number) => void;
}) {
  const { t } = useTranslation("managementOrg");
  const pages = Math.max(1, Math.ceil(count / PAGE_SIZE));
  return (
    <div className="flex h-10 shrink-0 items-center justify-between border-t border-border px-3 text-xs text-muted-foreground">
      <span>{t("count", { count })}</span>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="ghost"
          disabled={page <= 1}
          onClick={() => onPage(page - 1)}
        >
          {t("previous")}
        </Button>
        <span>{t("page", { page, pages })}</span>
        <Button
          size="sm"
          variant="ghost"
          disabled={page >= pages}
          onClick={() => onPage(page + 1)}
        >
          {t("next")}
        </Button>
      </div>
    </div>
  );
}

function useDirectory() {
  const [actor, setActor] = useState<AccountView | null>(null);
  const [organizations, setOrganizations] = useState<OrgEntry[]>([]);
  const [workspaces, setWorkspaces] = useState<WorkspaceEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const reload = useCallback(async () => {
    const id = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const [account, orgPage] = await Promise.all([
        api.getAccount(),
        api.listOrganizations(),
      ]);
      if (id !== request.current) return;
      setActor(account);
      const initialOrganizations: OrgEntry[] = orgPage.items.map(
        (organization) => ({
          organization,
          owner: null,
          workspaces: null,
          error: null,
        }),
      );
      setOrganizations(initialOrganizations);
      const orgEntries: OrgEntry[] = [];
      for (let start = 0; start < initialOrganizations.length; start += 4) {
        const batch = await Promise.all(
          initialOrganizations.slice(start, start + 4).map(async (entry) => {
            const [ws, people] = await Promise.allSettled([
              api.listWorkspaces(entry.organization.id),
              api.listOrganizationMembers(entry.organization.id),
            ]);
            const owner =
              people.status === "fulfilled"
                ? people.value.items.find(
                    (person) =>
                      person.accountId === entry.organization.accountId,
                  )
                : null;
            return {
              ...entry,
              owner: owner?.username || owner?.email || null,
              workspaces: ws.status === "fulfilled" ? ws.value.items : null,
              error: ws.status === "rejected" ? errorMessage(ws.reason) : null,
            };
          }),
        );
        if (id !== request.current) return;
        orgEntries.push(...batch);
        setOrganizations((current) =>
          current.map(
            (entry) =>
              batch.find(
                (updated) => updated.organization.id === entry.organization.id,
              ) ?? entry,
          ),
        );
      }
      const entries: WorkspaceEntry[] = orgEntries.flatMap((entry) =>
        (entry.workspaces ?? []).map((workspace) => ({
          organization: entry.organization,
          workspace,
          usage: null,
          usageError: null,
        })),
      );
      setWorkspaces(entries);
      // The inventory is usable now. Usage counts are secondary and must not
      // keep the whole management page in a loading state.
      setLoading(false);
      // Keep optional usage lookups bounded independently of tenant count.
      for (let start = 0; start < entries.length; start += 4) {
        const next = await Promise.all(
          entries.slice(start, start + 4).map(async (entry) => {
            try {
              return {
                ...entry,
                usage: await loadUsage(entry.workspace.id),
                usageError: null,
              };
            } catch (err) {
              return { ...entry, usage: null, usageError: errorMessage(err) };
            }
          }),
        );
        if (id !== request.current) return;
        setWorkspaces((current) =>
          current.map(
            (entry) =>
              next.find((item) => item.workspace.id === entry.workspace.id) ??
              entry,
          ),
        );
      }
    } catch (err) {
      if (id === request.current) setError(errorMessage(err));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void reload();
    return () => {
      request.current++;
    };
  }, [reload]);
  return { actor, organizations, workspaces, loading, error, reload };
}

export function OrganizationManagement({
  onDataChanged,
  platformView,
}: {
  onDataChanged?: () => void;
  platformView?: PlatformView;
}) {
  const { t } = useTranslation("managementOrg");
  const { actor, organizations, workspaces, loading, error, reload } =
    useDirectory();
  const stateKey = `buglogin:management:${platformView ?? "organization"}:v1`;
  const pendingDeleteKey = `${stateKey}:pending-delete:v1`;
  const [savedState] = useState(() => readManagementState(stateKey));
  const [pendingDeletes, setPendingDeletes] = useState(() =>
    readPendingTenancyDeletes(pendingDeleteKey),
  );
  const [initialUrlRoute] = useState(() =>
    typeof window === "undefined"
      ? null
      : parseOrganizationManagementRoute(window.location.search),
  );
  const [route, setRoute] = useState<Route>(
    initialUrlRoute
      ? {
          orgId: initialUrlRoute.organizationId,
          workspaceId: initialUrlRoute.workspaceId,
        }
      : savedState.route,
  );
  const [orgTab, setOrgTab] = useState<"workspaces" | "accounts" | "settings">(
    initialUrlRoute?.organizationTab ?? savedState.orgTab,
  );
  const skipHistoryPush = useRef(false);
  const [search, setSearch] = useState(savedState.search);
  const [page, setPage] = useState(savedState.page);
  const [selection, setSelection] = useState<string[]>([]);
  const [orgEditor, setOrgEditor] = useState<Organization | "new" | null>(null);
  const [workspaceEditor, setWorkspaceEditor] = useState<
    WorkspaceEntry | "new" | null
  >(null);
  const [bulk, setBulk] = useState<{
    mode: "create" | "quota";
    entries: WorkspaceEntry[];
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<
    Organization | WorkspaceEntry | null
  >(null);
  const [transferTarget, setTransferTarget] = useState<Organization | null>(
    null,
  );
  const [workspaceInitialTab, setWorkspaceInitialTab] =
    useState<ManagementWorkspaceTab>(
      initialUrlRoute?.workspaceTab ?? "overview",
    );
  useEffect(() => {
    try {
      window.sessionStorage.setItem(
        stateKey,
        JSON.stringify({ route, orgTab, search, page }),
      );
    } catch {
      // The current view still works when the desktop storage is unavailable.
    }
  }, [orgTab, page, route, search, stateKey]);
  useEffect(() => {
    try {
      if (pendingDeletes.length > 0) {
        window.sessionStorage.setItem(
          pendingDeleteKey,
          serializePendingTenancyDeletes(pendingDeletes),
        );
      } else {
        window.sessionStorage.removeItem(pendingDeleteKey);
      }
    } catch {
      // Cleanup remains usable in the current view if session storage is unavailable.
    }
  }, [pendingDeletes, pendingDeleteKey]);
  useEffect(() => {
    const onPopState = () => {
      skipHistoryPush.current = true;
      const urlRoute = parseOrganizationManagementRoute(window.location.search);
      setRoute({
        orgId: urlRoute?.organizationId ?? null,
        workspaceId: urlRoute?.workspaceId ?? null,
      });
      setOrgTab(urlRoute?.organizationTab ?? "workspaces");
      setWorkspaceInitialTab(urlRoute?.workspaceTab ?? "overview");
      setSelection([]);
    };
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);
  useEffect(() => {
    if (skipHistoryPush.current) {
      skipHistoryPush.current = false;
      return;
    }
    const query = serializeOrganizationManagementRoute(window.location.search, {
      organizationId: route.orgId,
      workspaceId: route.workspaceId,
      organizationTab: orgTab,
      workspaceTab: workspaceInitialTab,
    });
    const nextUrl = `${window.location.pathname}${query ? `?${query}` : ""}${window.location.hash}`;
    const currentUrl = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    if (nextUrl !== currentUrl)
      window.history.pushState(
        { ...window.history.state, bugloginOrganizationManagement: true },
        "",
        nextUrl,
      );
  }, [orgTab, route, workspaceInitialTab]);
  const [createAccounts, setCreateAccounts] = useState<{
    workspaceId?: string;
    bulk?: boolean;
  } | null>(null);
  const selectedOrg = organizations.find(
    (entry) => entry.organization.id === route.orgId,
  )?.organization;
  const selectedWs = workspaces.find(
    (entry) => entry.workspace.id === route.workspaceId,
  );
  const canManageOrg = useCallback(
    (org: Organization) =>
      Boolean(
        actor &&
          (actor.platformAdmin ||
            actor.manageableOrganizationIds.includes(org.id)),
      ),
    [actor],
  );
  const canManageWorkspace = (entry: WorkspaceEntry) =>
    canManageOrg(entry.organization) ||
    Boolean(actor?.manageableWorkspaceIds.includes(entry.workspace.id));
  const showOrgs = !platformView
    ? !selectedOrg
    : platformView === "organizations" && !selectedOrg;
  const filteredOrgs = organizations.filter((entry) =>
    `${entry.organization.name} ${entry.owner ?? ""}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  const filteredWorkspaces = workspaces.filter(
    (entry) =>
      (!selectedOrg || entry.organization.id === selectedOrg.id) &&
      `${entry.workspace.name} ${entry.organization.name}`
        .toLowerCase()
        .includes(search.toLowerCase()),
  );
  const visibleOrgs = filteredOrgs.slice(
    (page - 1) * PAGE_SIZE,
    page * PAGE_SIZE,
  );
  const visibleWorkspaces = filteredWorkspaces.slice(
    (page - 1) * PAGE_SIZE,
    page * PAGE_SIZE,
  );
  const selectionEntries = workspaces.filter((entry) =>
    selection.includes(entry.workspace.id),
  );
  const contextOrg =
    selectedOrg ??
    (workspaceEditor !== "new" && workspaceEditor
      ? workspaceEditor.organization
      : organizations.find((entry) => canManageOrg(entry.organization))
          ?.organization);

  const pendingDeleteBanner =
    pendingDeletes.length > 0 ? (
      <div
        role="status"
        aria-live="polite"
        className="mx-4 mt-3 space-y-2 rounded-md border border-border bg-muted/20 px-3 py-2"
      >
        <p className="text-sm font-medium">{t("pendingDeleteTitle")}</p>
        <ul className="space-y-2">
          {pendingDeletes.map((pending) => {
            const organization = organizations.find(
              (entry) => entry.organization.id === pending.organizationId,
            )?.organization;
            const workspace =
              pending.kind === "workspace"
                ? workspaces.find(
                    (entry) => entry.workspace.id === pending.workspaceId,
                  )
                : undefined;
            const canResume = Boolean(
              organization && (pending.kind === "organization" || workspace),
            );
            const name =
              pending.kind === "organization"
                ? pending.organizationName
                : pending.workspaceName;
            return (
              <li
                key={`${pending.kind}:${pending.organizationId}:${pending.kind === "workspace" ? pending.workspaceId : ""}`}
                className="flex flex-wrap items-center justify-between gap-3 border-t border-border/70 pt-2 first:border-0 first:pt-0"
              >
                <div className="min-w-0">
                  <p className="break-words text-xs text-muted-foreground">
                    {t("pendingDeleteBody", {
                      kind: t(
                        pending.kind === "organization"
                          ? "organizationLabel"
                          : "workspaceLabel",
                      ),
                      name,
                    })}
                  </p>
                  {!loading && !error && !canResume ? (
                    <p className="mt-1 text-xs text-destructive">
                      {t("pendingDeleteTargetUnavailable")}
                    </p>
                  ) : null}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={loading || !canResume}
                    onClick={() => {
                      if (!organization) return;
                      setRoute({ orgId: organization.id, workspaceId: null });
                      setOrgTab("workspaces");
                      if (pending.kind === "organization") {
                        setDeleteTarget(organization);
                      } else if (workspace) {
                        setDeleteTarget(workspace);
                      }
                    }}
                  >
                    <LuRefreshCw />
                    {t("resumeDelete")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      setPendingDeletes((current) =>
                        removePendingTenancyDelete(current, pending),
                      )
                    }
                  >
                    <LuX />
                    {t("dismissDelete")}
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      </div>
    ) : null;

  const previousPlatformView = useRef(platformView);
  useEffect(() => {
    if (previousPlatformView.current === platformView) return;
    previousPlatformView.current = platformView;
    setSelection([]);
    setPage(1);
    setSearch("");
    setRoute({ orgId: null, workspaceId: null });
  }, [platformView]);
  const changed = async () => {
    await reload();
    onDataChanged?.();
    window.dispatchEvent(new CustomEvent("buglogin:management-data-changed"));
  };
  const openOrg = (organization: Organization) => {
    setRoute({ orgId: organization.id, workspaceId: null });
    setOrgTab("workspaces");
    setSearch("");
    setPage(1);
    setSelection([]);
  };
  const openWs = (entry: WorkspaceEntry) => {
    setWorkspaceInitialTab("overview");
    setRoute({ orgId: entry.organization.id, workspaceId: entry.workspace.id });
    setSelection([]);
  };
  const toggle = (id: string) =>
    setSelection((current) =>
      current.includes(id)
        ? current.filter((value) => value !== id)
        : [...current, id],
    );
  const changeSearch = (value: string) => {
    setSearch(value);
    setPage(1);
    setSelection([]);
  };

  if (selectedWs)
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {pendingDeleteBanner}
        <ManagementWorkspace
          organizationId={selectedWs.organization.id}
          organizationName={selectedWs.organization.name}
          workspaceId={selectedWs.workspace.id}
          workspaceName={selectedWs.workspace.name}
          initialTab={workspaceInitialTab}
          onTabChange={setWorkspaceInitialTab}
          onBack={() =>
            setRoute((current) => ({ ...current, workspaceId: null }))
          }
          onDataChanged={() => {
            void changed();
          }}
        />
      </div>
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="shrink-0 border-b border-border px-4 py-3">
        <div className="flex min-w-0 items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              {selectedOrg ? (
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-7 shrink-0"
                  aria-label={t("back")}
                  onClick={() => {
                    setRoute({ orgId: null, workspaceId: null });
                    setSearch("");
                    setPage(1);
                    setSelection([]);
                  }}
                >
                  <LuArrowLeft />
                </Button>
              ) : null}
              <h1 className="truncate text-base font-semibold">
                {selectedOrg
                  ? selectedOrg.name
                  : platformView === "quotas"
                    ? t("quotas")
                    : showOrgs
                      ? t("organizations")
                      : t("workspaces")}
              </h1>
            </div>
            <p className="mt-1 truncate text-xs text-muted-foreground">
              {platformView === "quotas"
                ? t("quotaIntro")
                : showOrgs
                  ? t("orgIntro")
                  : t("workspaceIntro")}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              className="size-8"
              aria-label={t("refresh")}
              disabled={loading}
              onClick={() => void reload()}
            >
              <LuRefreshCw className={loading ? "animate-spin" : ""} />
            </Button>
            {showOrgs ? (
              <Button
                size="sm"
                onClick={() => setOrgEditor("new")}
                disabled={!actor}
              >
                <LuPlus />
                {t("createOrg")}
              </Button>
            ) : orgTab === "workspaces" || platformView ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    size="sm"
                    disabled={!contextOrg || !canManageOrg(contextOrg)}
                  >
                    <LuPlus />
                    {t("createWorkspace")}
                    <LuChevronDown />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => setWorkspaceEditor("new")}>
                    <LuPlus />
                    {t("createWorkspace")}
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={() => setBulk({ mode: "create", entries: [] })}
                  >
                    <LuBuilding2 />
                    {t("createManyWorkspaces")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </div>
        </div>
        {pendingDeleteBanner}
        {selectedOrg ? (
          <nav className="mt-3 flex gap-1" aria-label={selectedOrg.name}>
            {(["workspaces", "accounts", "settings"] as const).map((tab) => (
              <Button
                key={tab}
                size="sm"
                variant={orgTab === tab ? "secondary" : "ghost"}
                onClick={() => {
                  setOrgTab(tab);
                  setSelection([]);
                  setSearch("");
                }}
              >
                {tab === "accounts" ? t("directory") : t(tab)}
              </Button>
            ))}
          </nav>
        ) : null}
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden p-4">
        {error ? (
          <ErrorNotice message={error} onRetry={() => void reload()} />
        ) : null}
        {selectedOrg && orgTab === "accounts" ? (
          <AccountDirectory
            organizationId={selectedOrg.id}
            organizationOwnerId={selectedOrg.accountId}
            canManageOrganization={canManageOrg(selectedOrg)}
            onDataChanged={() => void changed()}
          />
        ) : selectedOrg && orgTab === "settings" ? (
          <div className="max-w-2xl space-y-5">
            <div className="flex items-start justify-between gap-4 border-b border-border pb-4">
              <div>
                <h2 className="text-sm font-medium">
                  {t("organizationSettings")}
                </h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  {selectedOrg.name}
                </p>
              </div>
              <Button
                size="sm"
                variant="outline"
                disabled={!canManageOrg(selectedOrg)}
                onClick={() => setOrgEditor(selectedOrg)}
              >
                <LuPencil />
                {t("editOrg")}
              </Button>
            </div>
            <div className="flex items-center justify-between gap-4">
              <div>
                <p className="text-sm font-medium">{t("owner")}</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {organizations.find(
                    (entry) => entry.organization.id === selectedOrg.id,
                  )?.owner ?? t("noIdentity")}
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={!canManageOrg(selectedOrg)}
                onClick={() => setTransferTarget(selectedOrg)}
              >
                <LuKeyRound />
                {t("transferOwner")}
              </Button>
            </div>
            <div className="flex items-center justify-between gap-4 border-t border-border pt-4">
              <p className="text-xs text-muted-foreground">
                {t("deleteOrgHelp")}
              </p>
              <Button
                variant="destructive"
                size="sm"
                disabled={!canManageOrg(selectedOrg)}
                onClick={() => setDeleteTarget(selectedOrg)}
              >
                <LuTrash2 />
                {t("deleteOrg")}
              </Button>
            </div>
          </div>
        ) : (
          <>
            <div className="mb-3 flex shrink-0 items-center gap-2">
              <div className="relative w-full max-w-md">
                <LuSearch className="pointer-events-none absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
                <Input
                  className="h-9 pl-9"
                  aria-label={t("search")}
                  placeholder={showOrgs ? t("searchOrg") : t("searchWorkspace")}
                  value={search}
                  onChange={(event) => changeSearch(event.target.value)}
                />
              </div>
              {search ? (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => changeSearch("")}
                >
                  {t("clearSearch")}
                </Button>
              ) : null}
              <span className="ml-auto whitespace-nowrap text-xs text-muted-foreground">
                {t("count", {
                  count: showOrgs
                    ? filteredOrgs.length
                    : filteredWorkspaces.length,
                })}
              </span>
            </div>
            {loading && organizations.length === 0 ? (
              <div
                role="status"
                className="space-y-2 rounded-md border border-border p-3"
              >
                {Array.from({ length: 5 }, (_, index) => (
                  <div
                    key={index}
                    className="h-10 animate-pulse rounded bg-muted"
                  />
                ))}
                <span className="sr-only">{t("loading")}</span>
              </div>
            ) : (
              <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-border">
                <div className="min-h-0 flex-1 overflow-auto">
                  <table className="w-full table-fixed text-left text-sm">
                    <thead className="sticky top-0 z-10 bg-muted">
                      <tr>
                        {showOrgs ? (
                          <>
                            <th className="w-1/3 px-3 py-2 text-xs font-medium text-muted-foreground">
                              {t("organization")}
                            </th>
                            <th className="hidden px-3 py-2 text-xs font-medium text-muted-foreground lg:table-cell">
                              {t("owner")}
                            </th>
                            <th className="w-36 px-3 py-2 text-xs font-medium text-muted-foreground">
                              {t("access")}
                            </th>
                            <th className="hidden w-24 px-3 py-2 text-right text-xs font-medium text-muted-foreground md:table-cell">
                              {t("workspaces")}
                            </th>
                            <th className="w-28 px-3 py-2 text-right text-xs font-medium text-muted-foreground">
                              {t("actions")}
                            </th>
                          </>
                        ) : (
                          <>
                            <th className="w-10 px-3 py-2">
                              <Checkbox
                                aria-label={t("selectAll")}
                                checked={
                                  visibleWorkspaces.length > 0 &&
                                  visibleWorkspaces.every((entry) =>
                                    selection.includes(entry.workspace.id),
                                  )
                                }
                                onCheckedChange={(checked) =>
                                  setSelection(
                                    checked
                                      ? visibleWorkspaces
                                          .filter(canManageWorkspace)
                                          .map((entry) => entry.workspace.id)
                                      : [],
                                  )
                                }
                              />
                            </th>
                            <th className="px-3 py-2 text-xs font-medium text-muted-foreground">
                              {t("workspace")}
                            </th>
                            <th className="w-32 px-3 py-2 text-right text-xs font-medium text-muted-foreground">
                              {t("profiles")}
                            </th>
                            <th className="w-32 px-3 py-2 text-right text-xs font-medium text-muted-foreground">
                              {t("members")}
                            </th>
                            <th className="w-36 px-3 py-2 text-right text-xs font-medium text-muted-foreground">
                              {t("actions")}
                            </th>
                          </>
                        )}
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border">
                      {showOrgs
                        ? visibleOrgs.map((entry) => (
                            <tr
                              key={entry.organization.id}
                              className="h-12 hover:bg-muted/40"
                            >
                              <td className="px-3 py-2">
                                <button
                                  type="button"
                                  className="flex w-full min-w-0 items-center gap-2 text-left font-medium hover:underline"
                                  onClick={() => openOrg(entry.organization)}
                                >
                                  <LuBuilding2 className="size-4 shrink-0 text-muted-foreground" />
                                  <span
                                    className="truncate"
                                    title={entry.organization.name}
                                  >
                                    {entry.organization.name}
                                  </span>
                                </button>
                                {entry.error ? (
                                  <p
                                    className="mt-0.5 truncate text-xs text-destructive"
                                    title={entry.error}
                                  >
                                    {entry.error}
                                  </p>
                                ) : null}
                              </td>
                              <td
                                className="hidden truncate px-3 py-2 text-xs text-muted-foreground lg:table-cell"
                                title={entry.owner ?? undefined}
                              >
                                {entry.owner ?? t("noIdentity")}
                              </td>
                              <td className="px-3 py-2">
                                <Badge
                                  variant="secondary"
                                  className="max-w-full truncate text-xs"
                                >
                                  {actor?.accountId ===
                                  entry.organization.accountId
                                    ? t("owned")
                                    : canManageOrg(entry.organization)
                                      ? t("managed")
                                      : t("view")}
                                </Badge>
                              </td>
                              <td className="hidden px-3 py-2 text-right font-mono text-xs md:table-cell">
                                {entry.workspaces?.length ?? "—"}
                              </td>
                              <td className="px-2 py-2">
                                <div className="flex justify-end gap-1">
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    onClick={() => openOrg(entry.organization)}
                                  >
                                    {t("enter")}
                                  </Button>
                                  <DropdownMenu>
                                    <DropdownMenuTrigger asChild>
                                      <Button
                                        size="icon"
                                        variant="ghost"
                                        className="size-8"
                                        aria-label={t("menu", {
                                          name: entry.organization.name,
                                        })}
                                      >
                                        <LuEllipsis />
                                      </Button>
                                    </DropdownMenuTrigger>
                                    <DropdownMenuContent align="end">
                                      <DropdownMenuItem
                                        onSelect={() =>
                                          openOrg(entry.organization)
                                        }
                                      >
                                        <LuArrowRight />
                                        {t("goWorkspaces")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        onSelect={() => {
                                          openOrg(entry.organization);
                                          setOrgTab("accounts");
                                        }}
                                      >
                                        <LuUsers />
                                        {t("directory")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        disabled={
                                          !canManageOrg(entry.organization)
                                        }
                                        onSelect={() =>
                                          setOrgEditor(entry.organization)
                                        }
                                      >
                                        <LuPencil />
                                        {t("editOrg")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        disabled={
                                          !canManageOrg(entry.organization)
                                        }
                                        onSelect={() =>
                                          setTransferTarget(entry.organization)
                                        }
                                      >
                                        <LuKeyRound />
                                        {t("transferOwner")}
                                      </DropdownMenuItem>
                                      <DropdownMenuSeparator />
                                      <DropdownMenuItem
                                        variant="destructive"
                                        disabled={
                                          !canManageOrg(entry.organization)
                                        }
                                        onSelect={() =>
                                          setDeleteTarget(entry.organization)
                                        }
                                      >
                                        <LuTrash2 />
                                        {t("deleteOrg")}
                                      </DropdownMenuItem>
                                    </DropdownMenuContent>
                                  </DropdownMenu>
                                </div>
                              </td>
                            </tr>
                          ))
                        : visibleWorkspaces.map((entry) => (
                            <tr
                              key={entry.workspace.id}
                              className="h-12 hover:bg-muted/40"
                              data-state={
                                selection.includes(entry.workspace.id)
                                  ? "selected"
                                  : undefined
                              }
                            >
                              <td className="px-3 py-2">
                                <Checkbox
                                  checked={selection.includes(
                                    entry.workspace.id,
                                  )}
                                  disabled={!canManageWorkspace(entry)}
                                  aria-label={t("selectRow", {
                                    name: entry.workspace.name,
                                  })}
                                  onCheckedChange={() =>
                                    toggle(entry.workspace.id)
                                  }
                                />
                              </td>
                              <td className="min-w-0 px-3 py-2">
                                <button
                                  type="button"
                                  className="block max-w-full truncate text-left font-medium hover:underline"
                                  title={entry.workspace.name}
                                  onClick={() => openWs(entry)}
                                >
                                  {entry.workspace.name}
                                </button>
                                <span className="block truncate text-xs text-muted-foreground">
                                  {entry.organization.name}
                                </span>
                              </td>
                              <td
                                className="px-3 py-2 text-right font-mono text-xs"
                                title={entry.usageError ?? undefined}
                              >
                                {entry.usage ? entry.usage.profiles : "—"} /{" "}
                                {entry.workspace.policy.maxProfiles}
                              </td>
                              <td
                                className="px-3 py-2 text-right font-mono text-xs"
                                title={entry.usageError ?? undefined}
                              >
                                {entry.usage ? entry.usage.members : "—"} /{" "}
                                {entry.workspace.policy.maxMembers}
                              </td>
                              <td className="px-2 py-2">
                                <div className="flex justify-end gap-1">
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    onClick={() => openWs(entry)}
                                  >
                                    {t("manage")}
                                  </Button>
                                  <DropdownMenu>
                                    <DropdownMenuTrigger asChild>
                                      <Button
                                        size="icon"
                                        variant="ghost"
                                        className="size-8"
                                        aria-label={t("menu", {
                                          name: entry.workspace.name,
                                        })}
                                      >
                                        <LuEllipsis />
                                      </Button>
                                    </DropdownMenuTrigger>
                                    <DropdownMenuContent align="end">
                                      <DropdownMenuItem
                                        onSelect={() => openWs(entry)}
                                      >
                                        <LuUsers />
                                        {t("members")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        disabled={!canManageWorkspace(entry)}
                                        onSelect={() =>
                                          setWorkspaceEditor(entry)
                                        }
                                      >
                                        <LuPencil />
                                        {t("editWorkspace")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        disabled={!canManageWorkspace(entry)}
                                        onSelect={() =>
                                          setBulk({
                                            mode: "quota",
                                            entries: [entry],
                                          })
                                        }
                                      >
                                        <LuSettings2 />
                                        {t("quotaWorkspace")}
                                      </DropdownMenuItem>
                                      <DropdownMenuItem
                                        disabled={!canManageWorkspace(entry)}
                                        onSelect={() => {
                                          setRoute({
                                            orgId: entry.organization.id,
                                            workspaceId: null,
                                          });
                                          setCreateAccounts({
                                            workspaceId: entry.workspace.id,
                                          });
                                        }}
                                      >
                                        <LuPlus />
                                        {t("createAccount")}
                                      </DropdownMenuItem>
                                      <DropdownMenuSeparator />
                                      <DropdownMenuItem
                                        variant="destructive"
                                        disabled={!canManageWorkspace(entry)}
                                        onSelect={() => setDeleteTarget(entry)}
                                      >
                                        <LuTrash2 />
                                        {t("deleteWorkspace")}
                                      </DropdownMenuItem>
                                    </DropdownMenuContent>
                                  </DropdownMenu>
                                </div>
                              </td>
                            </tr>
                          ))}
                    </tbody>
                  </table>
                  {(showOrgs
                    ? filteredOrgs.length
                    : filteredWorkspaces.length) === 0 ? (
                    <EmptyState
                      text={
                        search
                          ? t("noResults")
                          : showOrgs
                            ? t("empty")
                            : t("noWorkspaces")
                      }
                      action={
                        !showOrgs && contextOrg && canManageOrg(contextOrg) ? (
                          <Button
                            size="sm"
                            onClick={() => setWorkspaceEditor("new")}
                          >
                            <LuPlus />
                            {t("createFirst")}
                          </Button>
                        ) : undefined
                      }
                    />
                  ) : null}
                </div>
                <Pager
                  count={
                    showOrgs ? filteredOrgs.length : filteredWorkspaces.length
                  }
                  page={page}
                  onPage={setPage}
                />
              </div>
            )}
            {selectionEntries.length ? (
              <div className="mt-3 flex shrink-0 items-center gap-2 rounded-md border border-border bg-muted/40 px-3 py-2">
                <span className="text-xs font-medium">
                  {t("selected", { count: selectionEntries.length })}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setBulk({ mode: "quota", entries: selectionEntries })
                  }
                >
                  <LuSettings2 />
                  {t("changeQuotas")}
                </Button>
                <Button
                  className="ml-auto"
                  size="sm"
                  variant="ghost"
                  onClick={() => setSelection([])}
                >
                  <LuX />
                  {t("clearSelection")}
                </Button>
              </div>
            ) : null}
          </>
        )}
      </div>
      {orgEditor && actor ? (
        <OrganizationEditor
          organization={orgEditor === "new" ? undefined : orgEditor}
          accountId={actor.accountId}
          platform={actor.platformAdmin}
          onClose={() => setOrgEditor(null)}
          onSaved={() => void changed()}
        />
      ) : null}
      {workspaceEditor && contextOrg ? (
        <WorkspaceEditor
          entry={workspaceEditor === "new" ? undefined : workspaceEditor}
          organizations={organizations
            .filter((entry) => canManageOrg(entry.organization))
            .map((entry) => entry.organization)}
          organization={contextOrg}
          onClose={() => setWorkspaceEditor(null)}
          onSaved={() => void changed()}
        />
      ) : null}
      {bulk && contextOrg ? (
        <WorkspaceBatch
          mode={bulk.mode}
          entries={bulk.entries}
          organization={contextOrg}
          existingNames={workspaces
            .filter((entry) => entry.organization.id === contextOrg.id)
            .map((entry) => entry.workspace.name)}
          onClose={() => setBulk(null)}
          onSaved={() => void changed()}
        />
      ) : null}
      {deleteTarget ? (
        <DeleteTenancy
          target={deleteTarget}
          onClose={() => setDeleteTarget(null)}
          onNavigate={(destination) => {
            const organization =
              "workspace" in deleteTarget
                ? deleteTarget.organization
                : deleteTarget;
            setPendingDeletes((current) =>
              upsertPendingTenancyDelete(
                current,
                pendingTenancyDeleteFor(deleteTarget),
              ),
            );
            setDeleteTarget(null);
            if (typeof destination === "string") {
              setRoute({ orgId: organization.id, workspaceId: null });
              setOrgTab(destination);
            } else if ("workspace" in deleteTarget) {
              setWorkspaceInitialTab(destination.workspaceTab);
              setRoute({
                orgId: organization.id,
                workspaceId: deleteTarget.workspace.id,
              });
            }
          }}
          onSaved={() => {
            setPendingDeletes((current) =>
              removePendingTenancyDelete(
                current,
                pendingTenancyDeleteFor(deleteTarget),
              ),
            );
            setRoute({ orgId: null, workspaceId: null });
            void changed();
          }}
        />
      ) : null}
      {transferTarget ? (
        <TransferOwnership
          organization={transferTarget}
          onClose={() => setTransferTarget(null)}
          onManageMembers={() => {
            setRoute({ orgId: transferTarget.id, workspaceId: null });
            setOrgTab("accounts");
            setTransferTarget(null);
          }}
          onSaved={() => void changed()}
        />
      ) : null}
      {createAccounts && selectedOrg ? (
        <AccountProvisioning
          open
          onOpenChange={(open) => {
            if (!open) setCreateAccounts(null);
          }}
          organizationId={selectedOrg.id}
          workspaceId={createAccounts.workspaceId}
          initialMode={createAccounts.bulk ? "bulk" : "single"}
          onCreated={() => void changed()}
        />
      ) : null}
    </div>
  );
}

function OrganizationEditor({
  organization,
  accountId,
  platform,
  onClose,
  onSaved,
}: {
  organization?: Organization;
  accountId: string;
  platform: boolean;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [name, setName] = useState(organization?.name ?? "");
  const [limit, setLimit] = useState("10");
  const [record, setRecord] = useState<OrganizationRecord | null>(null);
  const [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(Boolean(organization));
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    if (!organization) return;
    void api
      .getOrganizationRecord({ organizationId: organization.id })
      .then((value) => {
        if (active) {
          setRecord(value);
          setName(value.organization.name);
          setLimit(String(value.policy.maxWorkspaces));
        }
      })
      .catch((err) => {
        if (active) setError(errorMessage(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [organization]);
  const save = async () => {
    if (!name.trim()) {
      setError(t("requiredName"));
      return;
    }
    if (!Number.isSafeInteger(Number(limit)) || Number(limit) < 1) {
      setError(t("invalidLimit"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (organization) {
        if (!record) throw new Error(t("errorTitle"));
        await api.updateOrganization({
          organizationId: organization.id,
          update: {
            organization: { ...record.organization, name: name.trim() },
            policy: {
              ...record.policy,
              ...(platform ? { maxWorkspaces: Number(limit) } : {}),
            },
            expectedRevision: record.revision,
          },
        });
      } else
        await api.createOrganization({
          organization: {
            id: `org_${crypto.randomUUID().replaceAll("-", "")}`,
            accountId,
            name: name.trim(),
            createdAt: new Date().toISOString(),
          },
          policy: { maxWorkspaces: Number(limit), offlineEntitled: false },
        });
      showSuccessToast(t(organization ? "edited" : "created"));
      onSaved();
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t(organization ? "editOrg" : "createOrg")}</DialogTitle>
          <DialogDescription>
            {organization ? t("ownerInfo") : t("createOrgHint")}
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
          className="space-y-4"
        >
          {error ? <ErrorNotice message={error} /> : null}
          <div className="space-y-1.5">
            <Label htmlFor="management-organization-name">{t("name")} *</Label>
            <Input
              id="management-organization-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={128}
              required
              disabled={busy || loading}
              autoFocus
            />
          </div>
          {platform ? (
            <div className="space-y-1.5">
              <Label htmlFor="management-organization-limit">
                {t("maxWorkspaces")}
              </Label>
              <Input
                id="management-organization-limit"
                type="number"
                min={1}
                max={10000}
                value={limit}
                onChange={(event) => setLimit(event.target.value)}
                disabled={busy || loading}
              />
              <p className="text-xs text-muted-foreground">
                {t("orgLimitHint")}
              </p>
            </div>
          ) : null}
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={onClose}
              disabled={busy}
            >
              {t("cancel")}
            </Button>
            <Button
              type="submit"
              disabled={busy || loading || (Boolean(organization) && !record)}
            >
              {busy || loading ? (
                <LuLoaderCircle className="animate-spin" />
              ) : (
                <LuCheck />
              )}
              {t(organization ? "save" : "create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function WorkspaceEditor({
  entry,
  organization,
  organizations,
  onClose,
  onSaved,
}: {
  entry?: WorkspaceEntry;
  organization: Organization;
  organizations: Organization[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [orgId, setOrgId] = useState(organization.id),
    [name, setName] = useState(entry?.workspace.name ?? ""),
    [profiles, setProfiles] = useState(
      String(entry?.workspace.policy.maxProfiles ?? 100),
    ),
    [members, setMembers] = useState(
      String(entry?.workspace.policy.maxMembers ?? 10),
    );
  const [defaultExtensionGroupId, setDefaultExtensionGroupId] = useState(
    entry?.workspace.policy.defaultExtensionGroupId ?? "none",
  );
  const [extensionGroups, setExtensionGroups] = useState<
    { id: string; name: string }[]
  >([]);
  const [record, setRecord] = useState<WorkspaceRecord | null>(null),
    [error, setError] = useState<string | null>(null),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(Boolean(entry));
  useEffect(() => {
    let active = true;
    if (!entry) return;
    void Promise.all([
      api.getWorkspaceRecord({
        organizationId: entry.organization.id,
        workspaceId: entry.workspace.id,
      }),
      api.listExtensionGroups(entry.workspace.id),
    ])
      .then(([value, groups]) => {
        if (active) {
          setRecord(value);
          setName(value.workspace.name);
          setProfiles(String(value.workspace.policy.maxProfiles));
          setMembers(String(value.workspace.policy.maxMembers));
          setDefaultExtensionGroupId(
            value.workspace.policy.defaultExtensionGroupId ?? "none",
          );
          setExtensionGroups(
            groups.map(({ group }) => ({ id: group.id, name: group.name })),
          );
        }
      })
      .catch((err) => {
        if (active) setError(errorMessage(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [entry]);
  const save = async () => {
    if (!name.trim()) {
      setError(t("requiredName"));
      return;
    }
    if (
      [profiles, members].some(
        (value) =>
          !Number.isSafeInteger(Number(value)) ||
          Number(value) < 1 ||
          Number(value) > 1_000_000,
      )
    ) {
      setError(t("invalidLimit"));
      return;
    }
    if (
      entry?.usage &&
      (Number(profiles) < entry.usage.profiles ||
        Number(members) < entry.usage.members)
    ) {
      setError(t("minimumUsage"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (entry) {
        if (!record) throw new Error(t("errorTitle"));
        await api.updateWorkspace({
          organizationId: entry.organization.id,
          workspaceId: entry.workspace.id,
          update: {
            workspace: {
              ...record.workspace,
              name: name.trim(),
              policy: {
                ...record.workspace.policy,
                maxProfiles: Number(profiles),
                maxMembers: Number(members),
                defaultExtensionGroupId:
                  defaultExtensionGroupId === "none"
                    ? null
                    : defaultExtensionGroupId,
              },
            },
            expectedRevision: record.revision,
          },
        });
      } else
        await api.createWorkspace({
          organizationId: orgId,
          workspace: {
            id: `ws_${crypto.randomUUID().replaceAll("-", "")}`,
            organizationId: orgId,
            name: name.trim(),
            createdAt: new Date().toISOString(),
            policy: {
              maxProfiles: Number(profiles),
              maxMembers: Number(members),
              maxStorageBytes: 1_073_741_824,
              offlineEntitled: false,
            },
          },
        });
      showSuccessToast(t(entry ? "edited" : "created"));
      onSaved();
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {t(entry ? "editWorkspace" : "createWorkspace")}
          </DialogTitle>
          <DialogDescription>
            {entry?.organization.name ?? organization.name}
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
          className="space-y-4"
        >
          {error ? <ErrorNotice message={error} /> : null}
          {!entry && organizations.length > 1 ? (
            <div className="space-y-1.5">
              <Label>{t("organization")}</Label>
              <Select value={orgId} onValueChange={setOrgId}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {organizations.map((org) => (
                    <SelectItem key={org.id} value={org.id}>
                      {org.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ) : null}
          <div className="space-y-1.5">
            <Label htmlFor="management-workspace-name">{t("name")} *</Label>
            <Input
              id="management-workspace-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={128}
              required
              disabled={busy || loading}
              autoFocus
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="management-profile-limit">
                {t("profileLimit")}
              </Label>
              <Input
                id="management-profile-limit"
                type="number"
                min={Math.max(1, entry?.usage?.profiles ?? 1)}
                value={profiles}
                onChange={(event) => setProfiles(event.target.value)}
                disabled={busy || loading}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="management-member-limit">
                {t("memberLimit")}
              </Label>
              <Input
                id="management-member-limit"
                type="number"
                min={Math.max(1, entry?.usage?.members ?? 1)}
                value={members}
                onChange={(event) => setMembers(event.target.value)}
                disabled={busy || loading}
              />
            </div>
          </div>
          {entry ? (
            <div className="space-y-1.5">
              <Label htmlFor="management-default-extension-group">
                {t("defaultExtensionGroup")}
              </Label>
              <Select
                value={defaultExtensionGroupId}
                onValueChange={setDefaultExtensionGroupId}
                disabled={busy || loading}
              >
                <SelectTrigger id="management-default-extension-group">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">
                    {t("noDefaultExtensionGroup")}
                  </SelectItem>
                  {extensionGroups.map((group) => (
                    <SelectItem key={group.id} value={group.id}>
                      {group.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ) : null}
          {entry ? (
            <p className="text-xs text-muted-foreground">
              {t("currentUsage")}:{" "}
              {entry.usage
                ? `${entry.usage.profiles} / ${entry.workspace.policy.maxProfiles} · ${entry.usage.members} / ${entry.workspace.policy.maxMembers}`
                : t("unavailable")}
            </p>
          ) : null}
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={onClose}
              disabled={busy}
            >
              {t("cancel")}
            </Button>
            <Button
              type="submit"
              disabled={busy || loading || (Boolean(entry) && !record)}
            >
              {busy || loading ? (
                <LuLoaderCircle className="animate-spin" />
              ) : (
                <LuCheck />
              )}
              {t(entry ? "save" : "create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function WorkspaceBatch({
  mode,
  entries,
  organization,
  existingNames,
  onClose,
  onSaved,
}: {
  mode: "create" | "quota";
  entries: WorkspaceEntry[];
  organization: Organization;
  existingNames: string[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [rows, setRows] = useState<BatchRow[]>(
    mode === "create"
      ? [{ key: crypto.randomUUID(), name: "", profiles: "100", members: "10" }]
      : [],
  );
  const [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(mode === "quota"),
    [review, setReview] = useState(false),
    [error, setError] = useState<string | null>(null),
    [batchKey, setBatchKey] = useState(crypto.randomUUID());
  const [allProfiles, setAllProfiles] = useState(""),
    [allMembers, setAllMembers] = useState("");
  const file = useRef<HTMLInputElement>(null);
  useEffect(() => {
    let active = true;
    if (mode !== "quota") return;
    void Promise.all(
      entries.map(async (entry) => ({
        key: entry.workspace.id,
        name: entry.workspace.name,
        profiles: String(entry.workspace.policy.maxProfiles),
        members: String(entry.workspace.policy.maxMembers),
        record: await api.getWorkspaceRecord({
          organizationId: entry.organization.id,
          workspaceId: entry.workspace.id,
        }),
        usage: entry.usage ?? (await loadUsage(entry.workspace.id)),
      })),
    )
      .then((value) => {
        if (active) setRows(value);
      })
      .catch((err) => {
        if (active) setError(errorMessage(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [entries, mode]);
  const rowError = (row: BatchRow) => {
    if (!row.name.trim()) return t("requiredName");
    if (
      [row.profiles, row.members].some(
        (value) =>
          !Number.isSafeInteger(Number(value)) ||
          Number(value) < 1 ||
          Number(value) > 1_000_000,
      )
    )
      return t("invalidLimit");
    if (
      mode === "create" &&
      (rows.filter(
        (other) =>
          other.name.trim().toLowerCase() === row.name.trim().toLowerCase(),
      ).length > 1 ||
        existingNames.some(
          (name) => name.toLowerCase() === row.name.trim().toLowerCase(),
        ))
    )
      return t("duplicate");
    if (
      mode === "quota" &&
      row.usage &&
      (Number(row.profiles) < row.usage.profiles ||
        Number(row.members) < row.usage.members)
    )
      return t("minimumUsage");
    return null;
  };
  const change = (key: string, patch: Partial<BatchRow>) => {
    setReview(false);
    setBatchKey(crypto.randomUUID());
    setRows((current) =>
      current.map((row) =>
        row.key === key
          ? { ...row, ...patch, status: undefined, error: undefined }
          : row,
      ),
    );
  };
  const importRows = (text: string) => {
    try {
      let cells = parseRows(text);
      if (cells[0]?.[0]?.toLowerCase() === "name") cells = cells.slice(1);
      if (cells.length > 100) throw new Error(t("maxBatch"));
      setRows(
        cells.map((values) => ({
          key: crypto.randomUUID(),
          name: values[0] ?? "",
          profiles: values[1] || "100",
          members: values[2] || "10",
        })),
      );
      setReview(false);
      setError(null);
      setBatchKey(crypto.randomUUID());
    } catch (err) {
      setError(errorMessage(err));
    }
  };
  const submit = async (retry = false) => {
    const targetRows = rows.filter((row) =>
      retry ? row.status === "failed" : row.status !== "succeeded",
    );
    if (targetRows.some(rowError)) {
      setError(t("invalidRows"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const items = targetRows.map((row) => {
        if (mode === "quota") {
          if (!row.record) throw new Error(t("errorTitle"));
          return {
            key: row.key,
            commandId: "workspace.update",
            input: {
              organizationId: row.record.workspace.organizationId,
              workspaceId: row.record.workspace.id,
              update: {
                workspace: {
                  ...row.record.workspace,
                  policy: {
                    ...row.record.workspace.policy,
                    maxProfiles: Number(row.profiles),
                    maxMembers: Number(row.members),
                  },
                },
                expectedRevision: row.record.revision,
              },
            },
          };
        }
        return {
          key: row.key,
          commandId: "workspace.create",
          input: {
            organizationId: organization.id,
            workspace: {
              id: `ws_${row.key.replaceAll("-", "")}`,
              organizationId: organization.id,
              name: row.name.trim(),
              policy: {
                maxProfiles: Number(row.profiles),
                maxMembers: Number(row.members),
                maxStorageBytes: 1_073_741_824,
                offlineEntitled: false,
              },
              createdAt: new Date().toISOString(),
            },
          },
        };
      });
      const result = await api.managementBatch({
        items,
        idempotencyKey: batchKey,
      });
      setRows((current) =>
        current.map((row) => {
          const receipt = result.items.find((item) => item.key === row.key);
          return receipt
            ? { ...row, status: receipt.status, error: receipt.error?.title }
            : row;
        }),
      );
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  const anyResults = rows.some((row) => row.status),
    invalid = rows.some(rowError),
    failed = rows.filter((row) => row.status === "failed").length;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="flex max-h-5/6 flex-col overflow-hidden sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>
            {t(mode === "create" ? "batchWorkspaceTitle" : "batchQuotaTitle")}
          </DialogTitle>
          <DialogDescription>{t("batchHelp")}</DialogDescription>
        </DialogHeader>
        {error ? <ErrorNotice message={error} /> : null}
        {mode === "create" ? (
          <div className="flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              variant="outline"
              onClick={() => file.current?.click()}
              disabled={busy || anyResults}
            >
              <LuFileUp />
              {t("importCsv")}
            </Button>
            <input
              ref={file}
              type="file"
              className="hidden"
              accept=".csv,.tsv,text/csv,text/tab-separated-values"
              onChange={(event) => {
                const selected = event.target.files?.[0];
                if (selected)
                  void selected
                    .text()
                    .then(importRows)
                    .catch((err) => setError(errorMessage(err)));
                event.target.value = "";
              }}
            />
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                const url = URL.createObjectURL(
                  new Blob(["name,profiles,members\r\n"], {
                    type: "text/csv;charset=utf-8",
                  }),
                );
                const link = document.createElement("a");
                link.href = url;
                link.download = "workspaces.csv";
                link.click();
                URL.revokeObjectURL(url);
              }}
            >
              <LuDownload />
              {t("downloadTemplate")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              className="ml-auto"
              disabled={rows.length >= 100 || busy || anyResults}
              onClick={() =>
                setRows((current) => [
                  ...current,
                  {
                    key: crypto.randomUUID(),
                    name: "",
                    profiles: "100",
                    members: "10",
                  },
                ])
              }
            >
              <LuPlus />
              {t("addRow")}
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-end gap-2">
            <div>
              <Label htmlFor="batch-profiles" className="text-xs">
                {t("profileLimit")}
              </Label>
              <Input
                id="batch-profiles"
                className="mt-1 h-8 w-28"
                type="number"
                min={1}
                value={allProfiles}
                onChange={(event) => setAllProfiles(event.target.value)}
              />
            </div>
            <div>
              <Label htmlFor="batch-members" className="text-xs">
                {t("memberLimit")}
              </Label>
              <Input
                id="batch-members"
                className="mt-1 h-8 w-28"
                type="number"
                min={1}
                value={allMembers}
                onChange={(event) => setAllMembers(event.target.value)}
              />
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={busy || anyResults}
              onClick={() => {
                setRows((current) =>
                  current.map((row) => ({
                    ...row,
                    profiles: allProfiles || row.profiles,
                    members: allMembers || row.members,
                  })),
                );
                setReview(false);
                setBatchKey(crypto.randomUUID());
              }}
            >
              {t("applyAll")}
            </Button>
            <p className="text-xs text-muted-foreground">{t("blankKeeps")}</p>
          </div>
        )}
        <div
          className="min-h-0 flex-1 overflow-auto rounded-md border border-border"
          onPaste={(event) => {
            if (
              mode === "create" &&
              !review &&
              !anyResults &&
              (event.clipboardData.getData("text").includes("\t") ||
                event.clipboardData.getData("text").includes("\n"))
            ) {
              event.preventDefault();
              importRows(event.clipboardData.getData("text"));
            }
          }}
        >
          <table className="w-full table-fixed text-left text-sm">
            <thead className="sticky top-0 bg-muted">
              <tr>
                <th className="px-3 py-2 text-xs font-medium">{t("name")}</th>
                <th className="w-36 px-3 py-2 text-xs font-medium">
                  {t("profileLimit")}
                </th>
                <th className="w-36 px-3 py-2 text-xs font-medium">
                  {t("memberLimit")}
                </th>
                <th className="w-48 px-3 py-2 text-xs font-medium">
                  {anyResults ? t("results") : t("review")}
                </th>
                {mode === "create" ? <th className="w-10" /> : null}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {rows.map((row) => (
                <tr key={row.key}>
                  <td className="px-2 py-2">
                    {mode === "create" && !review && !anyResults ? (
                      <Input
                        className="h-8"
                        aria-label={t("name")}
                        value={row.name}
                        onChange={(event) =>
                          change(row.key, { name: event.target.value })
                        }
                      />
                    ) : (
                      <div className="truncate font-medium" title={row.name}>
                        {row.name}
                      </div>
                    )}
                    {mode === "quota" && row.record ? (
                      <p className="mt-1 text-xs text-muted-foreground">
                        {t("currentUsage")}: {row.usage?.profiles ?? "—"} ·{" "}
                        {row.usage?.members ?? "—"}
                      </p>
                    ) : null}
                  </td>
                  <td className="px-2 py-2">
                    {review || anyResults ? (
                      <span className="font-mono text-xs">
                        {row.record
                          ? `${row.record.workspace.policy.maxProfiles} → `
                          : ""}
                        {row.profiles}
                      </span>
                    ) : (
                      <Input
                        className="h-8"
                        aria-label={`${t("profileLimit")} ${row.name}`}
                        type="number"
                        min={1}
                        value={row.profiles}
                        onChange={(event) =>
                          change(row.key, { profiles: event.target.value })
                        }
                      />
                    )}
                  </td>
                  <td className="px-2 py-2">
                    {review || anyResults ? (
                      <span className="font-mono text-xs">
                        {row.record
                          ? `${row.record.workspace.policy.maxMembers} → `
                          : ""}
                        {row.members}
                      </span>
                    ) : (
                      <Input
                        className="h-8"
                        aria-label={`${t("memberLimit")} ${row.name}`}
                        type="number"
                        min={1}
                        value={row.members}
                        onChange={(event) =>
                          change(row.key, { members: event.target.value })
                        }
                      />
                    )}
                  </td>
                  <td className="px-3 py-2 text-xs">
                    <span
                      className={
                        row.status === "failed" || rowError(row)
                          ? "text-destructive"
                          : "text-muted-foreground"
                      }
                    >
                      {row.status
                        ? t(row.status === "succeeded" ? "success" : "failed")
                        : (rowError(row) ?? t("valid"))}
                    </span>
                    {row.error ? (
                      <p className="mt-1 break-words text-destructive">
                        {row.error}
                      </p>
                    ) : null}
                  </td>
                  {mode === "create" ? (
                    <td>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="size-7"
                        aria-label={t("removeRow")}
                        disabled={busy || anyResults || review}
                        onClick={() =>
                          setRows((current) =>
                            current.filter((item) => item.key !== row.key),
                          )
                        }
                      >
                        <LuX />
                      </Button>
                    </td>
                  ) : null}
                </tr>
              ))}
            </tbody>
          </table>
          {loading ? (
            <div
              role="status"
              className="flex items-center justify-center gap-2 p-6 text-sm text-muted-foreground"
            >
              <LuLoaderCircle className="animate-spin" />
              {t("loading")}
            </div>
          ) : null}
        </div>
        <p className="text-xs text-muted-foreground">
          {anyResults
            ? t("resultSummary", {
                succeeded: rows.filter((row) => row.status === "succeeded")
                  .length,
                failed,
              })
            : mode === "create"
              ? t("pasteHelp")
              : t("usageHint")}
        </p>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            {t(anyResults ? "close" : "cancel")}
          </Button>
          {review && !anyResults ? (
            <Button
              variant="outline"
              onClick={() => setReview(false)}
              disabled={busy}
            >
              {t("back")}
            </Button>
          ) : null}
          {anyResults ? (
            failed > 0 ? (
              <Button onClick={() => void submit(true)} disabled={busy}>
                {busy ? <LuLoaderCircle className="animate-spin" /> : null}
                {t("retryFailed")}
              </Button>
            ) : null
          ) : (
            <Button
              disabled={
                busy ||
                loading ||
                rows.length === 0 ||
                invalid ||
                Boolean(error)
              }
              onClick={() => (review ? void submit() : setReview(true))}
            >
              {busy ? <LuLoaderCircle className="animate-spin" /> : <LuCheck />}
              {t(
                review
                  ? mode === "create"
                    ? "createCount"
                    : "applyCount"
                  : "inspect",
                { count: rows.length },
              )}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function DeleteTenancy({
  target,
  onClose,
  onSaved,
  onNavigate,
}: {
  target: Organization | WorkspaceEntry;
  onClose: () => void;
  onSaved: () => void;
  onNavigate: (
    destination:
      | "workspaces"
      | "accounts"
      | { workspaceTab: "members" | "groups" | "profiles" | "resources" },
  ) => void;
}) {
  const { t } = useTranslation("managementOrg");
  const workspace = "workspace" in target ? target : null;
  const org = workspace ? workspace.organization : (target as Organization);
  const name = workspace?.workspace.name ?? org.name;
  const [confirmation, setConfirmation] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null),
    [workspacePreflight, setWorkspacePreflight] = useState<Awaited<
      ReturnType<typeof api.getWorkspaceDeletePreflight>
    > | null>(null),
    [dependencyCheck, setDependencyCheck] = useState<Awaited<
      ReturnType<typeof api.getOrganizationDeletePreflight>
    > | null>(null),
    [checkingDependencies, setCheckingDependencies] = useState(!workspace),
    [dependencyError, setDependencyError] = useState<string | null>(null);
  const refreshDependencies = useCallback(async () => {
    setCheckingDependencies(true);
    setDependencyError(null);
    try {
      if (workspace) {
        setWorkspacePreflight(
          await api.getWorkspaceDeletePreflight({
            organizationId: org.id,
            workspaceId: workspace.workspace.id,
          }),
        );
        setDependencyCheck(null);
        return;
      }
      setDependencyCheck(
        await api.getOrganizationDeletePreflight({ organizationId: org.id }),
      );
    } catch (err) {
      setDependencyCheck(null);
      setWorkspacePreflight(null);
      setDependencyError(errorMessage(err));
    } finally {
      setCheckingDependencies(false);
    }
  }, [org.id, workspace]);
  useEffect(() => {
    void refreshDependencies();
  }, [refreshDependencies]);
  const blockedByDependencies = Boolean(
    checkingDependencies ||
      dependencyError ||
      (workspace
        ? !workspacePreflight?.canDelete
        : !dependencyCheck?.canDelete),
  );
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      if (workspace) {
        const preflight = await api.getWorkspaceDeletePreflight({
          organizationId: org.id,
          workspaceId: workspace.workspace.id,
        });
        setWorkspacePreflight(preflight);
        if (!preflight.canDelete) return;
        await api.deleteWorkspace({
          organizationId: org.id,
          workspaceId: workspace.workspace.id,
          expectedRevision: preflight.revision,
        });
      } else {
        const preflight = await api.getOrganizationDeletePreflight({
          organizationId: org.id,
        });
        setDependencyCheck(preflight);
        if (!preflight.canDelete) return;
        await api.deleteOrganization({
          organizationId: org.id,
          expectedRevision: preflight.revision,
        });
      }
      showSuccessToast(t("deleted"));
      onSaved();
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("deleteTitle")}</DialogTitle>
          <DialogDescription>
            {t(workspace ? "deleteWorkspaceHelp" : "deleteOrgHelp")}
          </DialogDescription>
        </DialogHeader>
        {error ? <ErrorNotice message={error} /> : null}
        {workspace ? (
          <section
            className="space-y-2 rounded-md border border-border bg-muted/20 p-3"
            aria-live="polite"
          >
            <p className="text-sm font-medium">{t("deleteDependencies")}</p>
            {checkingDependencies ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground">
                <LuLoaderCircle className="size-4 animate-spin" />
                {t("checkingWorkspaceDependencies")}
              </p>
            ) : dependencyError ? (
              <div className="space-y-2">
                <ErrorNotice message={dependencyError} />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void refreshDependencies()}
                >
                  <LuRefreshCw />
                  {t("retry")}
                </Button>
              </div>
            ) : workspacePreflight ? (
              workspacePreflight.canDelete ? (
                <p className="text-sm text-muted-foreground">
                  {t("workspaceDeleteReady")}
                </p>
              ) : (
                <div className="space-y-2">
                  <div className="space-y-1 text-sm text-muted-foreground">
                    {workspacePreflight.blockers.map((blocker) => (
                      <p key={blocker.code}>
                        {t(`deleteBlocker_${blocker.code}`, {
                          count: blocker.count,
                        })}
                      </p>
                    ))}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {workspacePreflight.blockers.some(
                      (item) => item.code === "workspace_has_profiles",
                    ) ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => onNavigate({ workspaceTab: "profiles" })}
                      >
                        <LuPanelsTopLeft />
                        {t("manageProfiles")}
                      </Button>
                    ) : null}
                    {workspacePreflight.blockers.some(
                      (item) => item.code === "workspace_has_resources",
                    ) ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() =>
                          onNavigate({ workspaceTab: "resources" })
                        }
                      >
                        <LuDatabase />
                        {t("manageResources")}
                      </Button>
                    ) : null}
                    {workspacePreflight.blockers.some(
                      (item) => item.code === "workspace_has_groups",
                    ) ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => onNavigate({ workspaceTab: "groups" })}
                      >
                        <LuFolderKanban />
                        {t("manageGroups")}
                      </Button>
                    ) : null}
                    {workspacePreflight.blockers.some((item) =>
                      ["workspace_has_teams", "workspace_has_members"].includes(
                        item.code,
                      ),
                    ) ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => onNavigate({ workspaceTab: "members" })}
                      >
                        <LuUsers />
                        {t("manageMembers")}
                      </Button>
                    ) : null}
                    {workspacePreflight.blockers.some((item) =>
                      [
                        "workspace_has_active_operations",
                        "workspace_has_sync_reservations",
                      ].includes(item.code),
                    ) ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => void refreshDependencies()}
                      >
                        <LuRefreshCw />
                        {t("retry")}
                      </Button>
                    ) : null}
                  </div>
                </div>
              )
            ) : null}
          </section>
        ) : (
          <section
            className="space-y-2 rounded-md border border-border bg-muted/20 p-3"
            aria-live="polite"
          >
            <p className="text-sm font-medium">{t("deleteDependencies")}</p>
            {checkingDependencies ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground">
                <LuLoaderCircle className="size-4 animate-spin" />
                {t("checkingDependencies")}
              </p>
            ) : dependencyError ? (
              <div className="space-y-2">
                <ErrorNotice message={dependencyError} />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void refreshDependencies()}
                >
                  <LuRefreshCw />
                  {t("retry")}
                </Button>
              </div>
            ) : dependencyCheck ? (
              <>
                <ul className="space-y-1 text-sm text-muted-foreground">
                  <li>
                    {t("organizationMemberCount", {
                      count: dependencyCheck.memberCount,
                    })}
                  </li>
                  <li>
                    {t("organizationWorkspaceCount", {
                      count: dependencyCheck.workspaceCount,
                    })}
                  </li>
                </ul>
                {blockedByDependencies ? (
                  <div className="flex flex-wrap gap-2 pt-1">
                    {dependencyCheck.memberCount > 0 ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => onNavigate("accounts")}
                      >
                        <LuUsers />
                        {t("directory")}
                      </Button>
                    ) : null}
                    {dependencyCheck.workspaceCount > 0 ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => onNavigate("workspaces")}
                      >
                        <LuBuilding2 />
                        {t("goWorkspaces")}
                      </Button>
                    ) : null}
                  </div>
                ) : null}
              </>
            ) : null}
          </section>
        )}
        <p className="break-words text-sm font-semibold">{name}</p>
        {!blockedByDependencies ? (
          <div className="space-y-1.5">
            <Label htmlFor="delete-tenancy-confirm">{t("confirmName")}</Label>
            <Input
              id="delete-tenancy-confirm"
              value={confirmation}
              onChange={(event) => setConfirmation(event.target.value)}
              autoComplete="off"
              disabled={busy}
            />
          </div>
        ) : null}
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            {t("cancel")}
          </Button>
          <Button
            variant="destructive"
            onClick={() => void remove()}
            disabled={busy || confirmation !== name || blockedByDependencies}
          >
            {busy ? <LuLoaderCircle className="animate-spin" /> : <LuTrash2 />}
            {t("delete")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function TransferOwnership({
  organization,
  onClose,
  onSaved,
  onManageMembers,
}: {
  organization: Organization;
  onClose: () => void;
  onSaved: () => void;
  onManageMembers: () => void;
}) {
  const { t } = useTranslation("managementOrg");
  const [people, setPeople] = useState<OrganizationMemberView[]>([]),
    [target, setTarget] = useState(""),
    [confirm, setConfirm] = useState(false),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(true),
    [reloadToken, setReloadToken] = useState(0),
    [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    if (reloadToken > 0) {
      setTarget("");
      setConfirm(false);
    }
    void api
      .listOrganizationMembers(organization.id)
      .then((page) => {
        if (active)
          setPeople(
            page.items.filter(
              (person) =>
                person.accountId !== organization.accountId &&
                person.status === "active",
            ),
          );
      })
      .catch((err) => {
        if (active) setError(errorMessage(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [organization.id, organization.accountId, reloadToken]);
  const transfer = async () => {
    setBusy(true);
    setError(null);
    try {
      const record = await api.getOrganizationRecord({
        organizationId: organization.id,
      });
      await api.transferOrganizationOwner({
        organizationId: organization.id,
        accountId: target,
        expectedRevision: record.revision,
      });
      showSuccessToast(t("transferred"));
      onSaved();
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("transferOwner")}</DialogTitle>
          <DialogDescription>{t("transferHelp")}</DialogDescription>
        </DialogHeader>
        {error ? <ErrorNotice message={error} /> : null}
        <p className="text-sm font-medium">{organization.name}</p>
        <div className="space-y-1.5">
          <Label>{t("newOwner")}</Label>
          {loading || people.length > 0 ? (
            <Select
              value={target}
              onValueChange={(value) => {
                setTarget(value);
                setConfirm(false);
              }}
              disabled={loading || busy || confirm}
            >
              <SelectTrigger>
                <SelectValue
                  placeholder={t(loading ? "loading" : "searchAccount")}
                />
              </SelectTrigger>
              <SelectContent>
                {people.map((person) => (
                  <SelectItem key={person.accountId} value={person.accountId}>
                    {person.username || person.email || t("unavailable")}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : null}
          {!loading && !error && people.length === 0 ? (
            <div className="space-y-2 rounded-md border border-border bg-muted/20 p-3">
              <p className="text-sm text-muted-foreground">
                {t("noEligibleAccounts")}
              </p>
              <Button size="sm" variant="outline" onClick={onManageMembers}>
                <LuUsers />
                {t("directory")}
              </Button>
            </div>
          ) : null}
          {error ? (
            <Button
              size="sm"
              variant="outline"
              disabled={loading || busy}
              onClick={() => setReloadToken((value) => value + 1)}
            >
              <LuRefreshCw className={loading ? "animate-spin" : undefined} />
              {t("retry")}
            </Button>
          ) : null}
        </div>
        {confirm ? (
          <div className="rounded-md border border-warning/40 bg-warning/5 px-3 py-2 text-sm">
            <p>
              {t("owner")} →{" "}
              {people.find((person) => person.accountId === target)?.username ||
                people.find((person) => person.accountId === target)?.email}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("transferHelp")}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("transferImpact")}
            </p>
          </div>
        ) : null}
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            {t("cancel")}
          </Button>
          <Button
            onClick={() => (confirm ? void transfer() : setConfirm(true))}
            disabled={!target || busy || loading}
          >
            {busy ? (
              <LuLoaderCircle className="animate-spin" />
            ) : (
              <LuKeyRound />
            )}
            {t(confirm ? "transfer" : "inspect")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

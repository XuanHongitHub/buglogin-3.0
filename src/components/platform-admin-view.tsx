"use client";

import type { FormEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuBuilding2,
  LuCircleAlert,
  LuHardDrive,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuServer,
  LuUsers,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type {
  AccountAdminRecord,
  AccountStatus,
  Organization,
  OrganizationPolicy,
  Resource,
  ResourceKind,
  Workspace,
} from "../../frontend/src/generated/command-registry";

const desktopApi = createTauriDesktopApi();

type WorkspaceEntry = {
  organization: Organization;
  workspace: Workspace;
};

type ResourceEntry = {
  organization: Organization;
  workspace: Workspace;
  resource: Resource;
};

type AdminSection = "organizations" | "workspaces" | "accounts" | "resources";

interface PlatformAdminViewProps {
  onNavigateHome: () => void;
  onOpenAuditLogs?: () => void;
}

const statusLabels: Record<AccountStatus, string> = {
  active: "Hoạt động",
  suspended: "Đình chỉ",
  disabled: "Vô hiệu hóa",
};

function ErrorState({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div
      className="flex items-center justify-between gap-3 border-b border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
      role="alert"
    >
      <span className="flex min-w-0 items-center gap-2">
        <LuCircleAlert className="size-4 shrink-0" />
        <span className="truncate">{message}</span>
      </span>
      <Button
        variant="outline"
        size="sm"
        className="h-7 shrink-0"
        onClick={onRetry}
      >
        Thử lại
      </Button>
    </div>
  );
}

export function PlatformAdminView({
  onNavigateHome,
  onOpenAuditLogs,
}: PlatformAdminViewProps) {
  const { t } = useTranslation();
  const [section, setSection] = useState<AdminSection>("organizations");
  const [search, setSearch] = useState("");
  const [organizations, setOrganizations] = useState<Organization[]>([]);
  const [workspaces, setWorkspaces] = useState<WorkspaceEntry[]>([]);
  const [accounts, setAccounts] = useState<AccountAdminRecord[]>([]);
  const [resources, setResources] = useState<ResourceEntry[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [accountsLoading, setAccountsLoading] = useState(true);
  const [resourcesLoading, setResourcesLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [resourceError, setResourceError] = useState<string | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [pendingAccount, setPendingAccount] = useState<{
    record: AccountAdminRecord;
    status: AccountStatus;
  } | null>(null);
  const [isUpdatingAccount, setIsUpdatingAccount] = useState(false);
  const [createAccountOpen, setCreateAccountOpen] = useState(false);
  const [createOrganizationOpen, setCreateOrganizationOpen] = useState(false);
  const [organizationName, setOrganizationName] = useState("");
  const [organizationLimit, setOrganizationLimit] = useState(10);
  const [isCreatingOrganization, setIsCreatingOrganization] = useState(false);
  const [editingOrganization, setEditingOrganization] =
    useState<Organization | null>(null);
  const [editingOrganizationName, setEditingOrganizationName] = useState("");
  const [editingOrganizationLimit, setEditingOrganizationLimit] = useState(10);
  const [editingOrganizationRevision, setEditingOrganizationRevision] =
    useState(0);
  const [editingOrganizationPolicy, setEditingOrganizationPolicy] =
    useState<OrganizationPolicy>({ maxWorkspaces: 10, offlineEntitled: false });
  const [isSavingOrganization, setIsSavingOrganization] = useState(false);
  const [newUsername, setNewUsername] = useState("");
  const [newEmail, setNewEmail] = useState("");
  const [isCreatingAccount, setIsCreatingAccount] = useState(false);
  const [createResourceOpen, setCreateResourceOpen] = useState(false);
  const [resourceWorkspaceId, setResourceWorkspaceId] = useState("");
  const [resourceName, setResourceName] = useState("");
  const [resourceKind, setResourceKind] = useState<ResourceKind>("proxy");
  const [isCreatingResource, setIsCreatingResource] = useState(false);
  const [quotaWorkspace, setQuotaWorkspace] = useState<WorkspaceEntry | null>(
    null,
  );
  const [maxProfiles, setMaxProfiles] = useState(0);
  const [maxMembers, setMaxMembers] = useState(0);
  const [isSavingQuota, setIsSavingQuota] = useState(false);
  const directoryRequest = useRef(0);
  const accountRequest = useRef(0);
  const resourceRequest = useRef(0);

  const loadDirectory = useCallback(async (refresh = false) => {
    const request = ++directoryRequest.current;
    setIsLoading(true);
    if (refresh) setIsRefreshing(true);
    try {
      const organizationPage = await desktopApi.listOrganizations();
      const workspacePages = await Promise.all(
        organizationPage.items.map((organization) =>
          desktopApi.listWorkspaces(organization.id),
        ),
      );
      if (request !== directoryRequest.current) return;
      setOrganizations(organizationPage.items);
      setWorkspaces(
        workspacePages.flatMap((page, index) =>
          page.items.map((workspace) => ({
            organization: organizationPage.items[index],
            workspace,
          })),
        ),
      );
      setLoadError(null);
    } catch (error) {
      if (request === directoryRequest.current)
        setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      if (request === directoryRequest.current) {
        setIsLoading(false);
        setIsRefreshing(false);
      }
    }
  }, []);

  const loadAccounts = useCallback(async () => {
    const request = ++accountRequest.current;
    setAccountsLoading(true);
    try {
      const page = await desktopApi.listAccounts();
      if (request !== accountRequest.current) return;
      setAccounts(page.items);
      setAccountError(null);
    } catch (error) {
      if (request === accountRequest.current)
        setAccountError(error instanceof Error ? error.message : String(error));
    } finally {
      if (request === accountRequest.current) setAccountsLoading(false);
    }
  }, []);

  const loadResources = useCallback(async () => {
    const request = ++resourceRequest.current;
    setResourcesLoading(true);
    try {
      const pages = await Promise.all(
        workspaces.map(async ({ organization, workspace }) => {
          const items: Resource[] = [];
          let cursor: string | null | undefined;
          for (let pageCount = 0; pageCount < 1000; pageCount += 1) {
            const page = await desktopApi.listResources({
              workspaceId: workspace.id,
              cursor,
              limit: 100,
            });
            items.push(...page.items);
            cursor = page.nextCursor;
            if (!cursor) break;
          }
          return items.map((resource) => ({
            organization,
            workspace,
            resource,
          }));
        }),
      );
      if (request !== resourceRequest.current) return;
      setResources(pages.flat());
      setResourceError(null);
    } catch (error) {
      if (request === resourceRequest.current)
        setResourceError(
          error instanceof Error ? error.message : String(error),
        );
    } finally {
      if (request === resourceRequest.current) setResourcesLoading(false);
    }
  }, [workspaces]);

  useEffect(() => {
    void Promise.all([loadDirectory(), loadAccounts()]);
    return () => {
      directoryRequest.current += 1;
      accountRequest.current += 1;
      resourceRequest.current += 1;
    };
  }, [loadAccounts, loadDirectory]);

  useEffect(() => {
    if (section === "resources" && workspaces.length > 0) void loadResources();
  }, [loadResources, section, workspaces.length]);

  const filteredOrganizations = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return organizations.filter((organization) =>
      !query ? true : organization.name.toLocaleLowerCase().includes(query),
    );
  }, [organizations, search]);

  const filteredWorkspaces = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return workspaces.filter(
      ({ organization, workspace }) =>
        !query ||
        workspace.name.toLocaleLowerCase().includes(query) ||
        organization.name.toLocaleLowerCase().includes(query),
    );
  }, [search, workspaces]);

  const filteredAccounts = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return accounts.filter(
      ({ account, identity }) =>
        !query ||
        [identity?.username, identity?.email, account.status]
          .filter(Boolean)
          .some((value) => value?.toLocaleLowerCase().includes(query)),
    );
  }, [accounts, search]);

  const filteredResources = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return resources.filter(
      ({ organization, workspace, resource }) =>
        !query ||
        [
          resource.displayName,
          resource.kind,
          resource.state,
          workspace.name,
          organization.name,
        ].some((value) => value.toLocaleLowerCase().includes(query)),
    );
  }, [resources, search]);

  const refreshCurrent = async () => {
    if (section === "accounts") return loadAccounts();
    if (section === "resources") return loadResources();
    return loadDirectory(true);
  };

  const updateAccountStatus = async () => {
    if (!pendingAccount) return;
    setIsUpdatingAccount(true);
    try {
      const { account } = pendingAccount.record;
      const updated = await desktopApi.updateAccountStatus({
        accountId: account.id,
        status: pendingAccount.status,
        expectedRevision: account.revision,
      });
      setAccounts((current) =>
        current.map((record) =>
          record.account.id === updated.id
            ? { ...record, account: updated }
            : record,
        ),
      );
      showSuccessToast("Đã cập nhật trạng thái tài khoản.");
      setPendingAccount(null);
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
      await loadAccounts();
    } finally {
      setIsUpdatingAccount(false);
    }
  };

  const createAccount = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const username = newUsername.trim();
    const email = newEmail.trim();
    if (!username || !email || isCreatingAccount) return;
    setIsCreatingAccount(true);
    try {
      const accountId = `acct_${crypto.randomUUID().replaceAll("-", "")}`;
      await desktopApi.createPendingAccount(
        { accountId, username, email },
        `platform-account-${crypto.randomUUID()}`,
      );
      showSuccessToast("Đã tạo tài khoản nền tảng.");
      setNewUsername("");
      setNewEmail("");
      setCreateAccountOpen(false);
      await loadAccounts();
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCreatingAccount(false);
    }
  };

  const createOrganization = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = organizationName.trim();
    if (!name || isCreatingOrganization) return;
    setIsCreatingOrganization(true);
    try {
      const account = await desktopApi.getAccount();
      await desktopApi.createOrganization({
        organization: {
          id: `org_${crypto.randomUUID().replaceAll("-", "")}`,
          accountId: account.accountId,
          name,
          createdAt: new Date().toISOString(),
        },
        policy: { maxWorkspaces: organizationLimit, offlineEntitled: false },
      });
      showSuccessToast("Đã tạo organization.");
      setOrganizationName("");
      setCreateOrganizationOpen(false);
      await loadDirectory(true);
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCreatingOrganization(false);
    }
  };

  const openOrganizationEditor = async (organization: Organization) => {
    try {
      const record = await desktopApi.getOrganizationRecord({
        organizationId: organization.id,
      });
      setEditingOrganization(record.organization);
      setEditingOrganizationName(record.organization.name);
      setEditingOrganizationLimit(record.policy.maxWorkspaces);
      setEditingOrganizationRevision(record.revision);
      setEditingOrganizationPolicy(record.policy);
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
    }
  };

  const saveOrganization = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!editingOrganization || isSavingOrganization) return;
    setIsSavingOrganization(true);
    try {
      await desktopApi.updateOrganization({
        organizationId: editingOrganization.id,
        update: {
          organization: {
            ...editingOrganization,
            name: editingOrganizationName.trim(),
          },
          policy: {
            ...editingOrganizationPolicy,
            maxWorkspaces: editingOrganizationLimit,
          },
          expectedRevision: editingOrganizationRevision,
        },
      });
      showSuccessToast("Đã cập nhật organization.");
      setEditingOrganization(null);
      await loadDirectory(true);
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
      await loadDirectory(true);
    } finally {
      setIsSavingOrganization(false);
    }
  };

  const createResource = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const workspace = workspaces.find(
      (entry) => entry.workspace.id === resourceWorkspaceId,
    );
    const displayName = resourceName.trim();
    if (!workspace || !displayName || isCreatingResource) return;
    setIsCreatingResource(true);
    try {
      await desktopApi.registerResource({
        resource: {
          id: `res_${crypto.randomUUID().replaceAll("-", "")}`,
          workspaceId: workspace.workspace.id,
          kind: resourceKind,
          state: "active",
          displayName,
        },
      });
      showSuccessToast("Đã đăng ký tài nguyên trong workspace.");
      setResourceName("");
      setCreateResourceOpen(false);
      await loadResources();
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCreatingResource(false);
    }
  };

  const saveQuota = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!quotaWorkspace || isSavingQuota) return;
    setIsSavingQuota(true);
    try {
      await desktopApi.updateWorkspace({
        organizationId: quotaWorkspace.organization.id,
        workspaceId: quotaWorkspace.workspace.id,
        update: {
          name: quotaWorkspace.workspace.name,
          policy: {
            ...quotaWorkspace.workspace.policy,
            maxProfiles,
            maxMembers,
          },
        },
      });
      showSuccessToast("Đã cập nhật hạn mức workspace.");
      setQuotaWorkspace(null);
      await loadDirectory(true);
    } catch (error) {
      showErrorToast(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSavingQuota(false);
    }
  };

  const countWorkspaces = (organizationId: string) =>
    workspaces.filter((entry) => entry.organization.id === organizationId)
      .length;

  const TableState = ({
    count,
    colSpan = 5,
  }: {
    count: number;
    colSpan?: number;
  }) => (
    <tbody>
      {((section === "organizations" || section === "workspaces") &&
        isLoading) ||
      (section === "accounts" && accountsLoading) ||
      (section === "resources" && resourcesLoading) ? (
        <tr>
          <td
            colSpan={colSpan}
            className="h-24 text-center text-muted-foreground"
          >
            Đang tải dữ liệu…
          </td>
        </tr>
      ) : count === 0 ? (
        <tr>
          <td
            colSpan={colSpan}
            className="h-24 text-center text-muted-foreground"
          >
            {search ? "Không có kết quả phù hợp." : "Chưa có dữ liệu."}
          </td>
        </tr>
      ) : null}
    </tbody>
  );

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
      <header className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2.5">
        <div className="flex min-w-0 items-center gap-3">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onNavigateHome}
            aria-label="Quay lại Profiles"
            className="size-8"
          >
            <LuServer className="size-4" />
          </Button>
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold">
              {t("platformAdmin.title", { defaultValue: "Platform Admin" })}
            </h1>
            <p className="truncate text-xs text-muted-foreground">
              Quản lý danh bạ nền tảng, tài khoản và tài nguyên theo phạm vi
              được API cấp.
            </p>
          </div>
          <Badge variant="outline" className="hidden shrink-0 sm:inline-flex">
            Theo quyền được cấp
          </Badge>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {onOpenAuditLogs && (
            <Button
              variant="outline"
              size="sm"
              className="h-8"
              onClick={onOpenAuditLogs}
            >
              Audit
            </Button>
          )}
          {section === "accounts" && (
            <Button
              size="sm"
              className="h-8 gap-1.5"
              onClick={() => setCreateAccountOpen(true)}
            >
              <LuPlus className="size-3.5" />
              Tạo account
            </Button>
          )}
          {section === "organizations" && (
            <Button
              size="sm"
              className="h-8 gap-1.5"
              onClick={() => setCreateOrganizationOpen(true)}
            >
              <LuPlus className="size-3.5" /> Tạo organization
            </Button>
          )}
          {section === "resources" && (
            <Button
              size="sm"
              className="h-8 gap-1.5"
              onClick={() => {
                setResourceWorkspaceId(workspaces[0]?.workspace.id ?? "");
                setCreateResourceOpen(true);
              }}
              disabled={workspaces.length === 0}
            >
              <LuPlus className="size-3.5" />
              Đăng ký resource
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5"
            onClick={() => void refreshCurrent()}
            disabled={isRefreshing || isLoading}
          >
            <LuRefreshCw
              className={cn(
                "size-3.5",
                (isRefreshing || isLoading) && "animate-spin",
              )}
            />
            Làm mới
          </Button>
        </div>
      </header>

      <Tabs
        value={section}
        onValueChange={(value) => {
          setSection(value as AdminSection);
          setSearch("");
        }}
        className="flex min-h-0 flex-1 flex-col"
      >
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2">
          <TabsList className="h-8 justify-start gap-1 bg-transparent p-0">
            <TabsTrigger value="organizations" className="h-8 px-2.5 text-xs">
              <LuBuilding2 className="mr-1.5 size-3.5" />
              Organizations
            </TabsTrigger>
            <TabsTrigger value="workspaces" className="h-8 px-2.5 text-xs">
              <LuHardDrive className="mr-1.5 size-3.5" />
              Workspaces
            </TabsTrigger>
            <TabsTrigger value="accounts" className="h-8 px-2.5 text-xs">
              <LuUsers className="mr-1.5 size-3.5" />
              Accounts{" "}
              <span className="ml-1 tabular-nums">{accounts.length}</span>
            </TabsTrigger>
            <TabsTrigger value="resources" className="h-8 px-2.5 text-xs">
              <LuServer className="mr-1.5 size-3.5" />
              Resources
            </TabsTrigger>
          </TabsList>
          <div className="relative w-64 max-w-[40vw]">
            <LuSearch className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Tìm trong danh sách…"
              className="h-8 pl-8 text-xs"
            />
          </div>
        </div>

        {loadError && section !== "accounts" && section !== "resources" && (
          <ErrorState
            message={loadError}
            onRetry={() => void loadDirectory(true)}
          />
        )}
        {accountError && section === "accounts" && (
          <ErrorState
            message={accountError}
            onRetry={() => void loadAccounts()}
          />
        )}
        {resourceError && section === "resources" && (
          <ErrorState
            message={resourceError}
            onRetry={() => void loadResources()}
          />
        )}

        <TabsContent
          value="organizations"
          className="m-0 min-h-0 flex-1 overflow-auto focus-visible:ring-0"
        >
          <table className="w-full min-w-[560px] text-left text-xs">
            <thead className="sticky top-0 z-10 border-b border-border bg-muted/70 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Organization</th>
                <th className="px-3 py-2">Workspaces</th>
                <th className="px-3 py-2">Created</th>
                <th className="px-3 py-2 text-right">Thao tác</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {filteredOrganizations.map((organization) => (
                <tr key={organization.id} className="h-10 hover:bg-muted/30">
                  <td className="px-3 font-medium">{organization.name}</td>
                  <td className="px-3 tabular-nums">
                    {countWorkspaces(organization.id)}
                  </td>
                  <td className="px-3 text-muted-foreground">
                    {new Date(organization.createdAt).toLocaleDateString()}
                  </td>
                  <td className="px-3 text-right">
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 gap-1.5"
                      onClick={() => void openOrganizationEditor(organization)}
                    >
                      <LuPencil className="size-3.5" /> Chỉnh sửa
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
            {isLoading && (
              <TableState count={filteredOrganizations.length} colSpan={4} />
            )}
            {!isLoading && filteredOrganizations.length === 0 && (
              <TableState count={0} colSpan={4} />
            )}
          </table>
        </TabsContent>

        <TabsContent
          value="workspaces"
          className="m-0 min-h-0 flex-1 overflow-auto focus-visible:ring-0"
        >
          <table className="w-full min-w-[760px] text-left text-xs">
            <thead className="sticky top-0 z-10 border-b border-border bg-muted/70 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Workspace</th>
                <th className="px-3 py-2">Organization</th>
                <th className="px-3 py-2 text-right">Profile limit</th>
                <th className="px-3 py-2 text-right">Member limit</th>
                <th className="px-3 py-2">Offline</th>
                <th className="px-3 py-2 text-right">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {filteredWorkspaces.map((entry) => {
                const { organization, workspace } = entry;
                return (
                  <tr key={workspace.id} className="h-10 hover:bg-muted/30">
                    <td className="px-3 font-medium">{workspace.name}</td>
                    <td className="px-3 text-muted-foreground">
                      {organization.name}
                    </td>
                    <td className="px-3 text-right font-mono tabular-nums">
                      {workspace.policy.maxProfiles.toLocaleString()}
                    </td>
                    <td className="px-3 text-right font-mono tabular-nums">
                      {workspace.policy.maxMembers.toLocaleString()}
                    </td>
                    <td className="px-3">
                      {workspace.policy.offlineEntitled ? "Được phép" : "Không"}
                    </td>
                    <td className="px-3 text-right">
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-7"
                        onClick={() => {
                          setQuotaWorkspace(entry);
                          setMaxProfiles(workspace.policy.maxProfiles);
                          setMaxMembers(workspace.policy.maxMembers);
                        }}
                      >
                        Hạn mức
                      </Button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
            {isLoading && (
              <TableState count={filteredWorkspaces.length} colSpan={6} />
            )}
            {!isLoading && filteredWorkspaces.length === 0 && (
              <TableState count={0} colSpan={6} />
            )}
          </table>
        </TabsContent>

        <TabsContent
          value="accounts"
          className="m-0 min-h-0 flex-1 overflow-auto focus-visible:ring-0"
        >
          <table className="w-full min-w-[760px] text-left text-xs">
            <thead className="sticky top-0 z-10 border-b border-border bg-muted/70 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Username</th>
                <th className="px-3 py-2">Email</th>
                <th className="px-3 py-2">Verification</th>
                <th className="px-3 py-2">Status</th>
                <th className="px-3 py-2 text-right">Action</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {filteredAccounts.map((record) => (
                <tr key={record.account.id} className="h-10 hover:bg-muted/30">
                  <td className="px-3 font-medium">
                    {record.identity?.username ?? "Không khả dụng"}
                  </td>
                  <td className="px-3">
                    {record.identity?.email ?? "Không khả dụng"}
                  </td>
                  <td className="px-3 text-muted-foreground">
                    {record.identity?.verification ?? "Không khả dụng"}
                  </td>
                  <td className="px-3">
                    <Badge
                      variant={
                        record.account.status === "active"
                          ? "secondary"
                          : "outline"
                      }
                    >
                      {statusLabels[record.account.status]}
                    </Badge>
                  </td>
                  <td className="px-3 text-right">
                    <Select
                      value={record.account.status}
                      onValueChange={(value) => {
                        const status = value as AccountStatus;
                        if (status !== record.account.status)
                          setPendingAccount({ record, status });
                      }}
                    >
                      <SelectTrigger
                        aria-label={`Trạng thái tài khoản ${record.identity?.username ?? ""}`}
                        className="ml-auto h-7 w-36 text-xs"
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="active">Hoạt động</SelectItem>
                        <SelectItem value="suspended">Đình chỉ</SelectItem>
                        <SelectItem value="disabled">Vô hiệu hóa</SelectItem>
                      </SelectContent>
                    </Select>
                  </td>
                </tr>
              ))}
            </tbody>
            {filteredAccounts.length === 0 && !accountError && (
              <TableState count={0} />
            )}
          </table>
        </TabsContent>

        <TabsContent
          value="resources"
          className="m-0 min-h-0 flex-1 overflow-auto focus-visible:ring-0"
        >
          <table className="w-full min-w-[760px] text-left text-xs">
            <thead className="sticky top-0 z-10 border-b border-border bg-muted/70 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Resource</th>
                <th className="px-3 py-2">Type</th>
                <th className="px-3 py-2">State</th>
                <th className="px-3 py-2">Workspace</th>
                <th className="px-3 py-2">Organization</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {filteredResources.map(
                ({ organization, workspace, resource }) => (
                  <tr
                    key={`${workspace.id}:${resource.id}`}
                    className="h-10 hover:bg-muted/30"
                  >
                    <td className="px-3 font-medium">{resource.displayName}</td>
                    <td className="px-3 capitalize">{resource.kind}</td>
                    <td className="px-3">
                      <Badge
                        variant={
                          resource.state === "active" ? "secondary" : "outline"
                        }
                      >
                        {resource.state}
                      </Badge>
                    </td>
                    <td className="px-3">{workspace.name}</td>
                    <td className="px-3 text-muted-foreground">
                      {organization.name}
                    </td>
                  </tr>
                ),
              )}
            </tbody>
            {workspaces.length > 0 &&
              resources.length === 0 &&
              !resourceError && <TableState count={0} />}
            {workspaces.length === 0 && !isLoading && <TableState count={0} />}
          </table>
        </TabsContent>
      </Tabs>

      <Dialog
        open={createOrganizationOpen}
        onOpenChange={setCreateOrganizationOpen}
      >
        <DialogContent className="max-w-md p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">Tạo organization</DialogTitle>
            <DialogDescription className="text-xs">
              Organization mới được tạo trong phạm vi tài khoản hiện tại; quyền
              được API xác thực.
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => void createOrganization(event)}
            className="space-y-3"
          >
            <div className="space-y-1">
              <Label htmlFor="platform-organization-name">
                Tên organization
              </Label>
              <Input
                id="platform-organization-name"
                value={organizationName}
                onChange={(event) => setOrganizationName(event.target.value)}
                required
                maxLength={120}
                autoFocus
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="platform-organization-limit">
                Giới hạn workspace
              </Label>
              <Input
                id="platform-organization-limit"
                type="number"
                min={1}
                max={10000}
                value={organizationLimit}
                onChange={(event) =>
                  setOrganizationLimit(Number(event.target.value))
                }
                required
              />
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => setCreateOrganizationOpen(false)}
                disabled={isCreatingOrganization}
              >
                Hủy
              </Button>
              <Button
                type="submit"
                disabled={!organizationName.trim() || isCreatingOrganization}
              >
                {isCreatingOrganization ? "Đang tạo…" : "Tạo organization"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog
        open={!!editingOrganization}
        onOpenChange={(open) => !open && setEditingOrganization(null)}
      >
        <DialogContent className="max-w-md p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">
              Chỉnh sửa organization
            </DialogTitle>
            <DialogDescription className="text-xs">
              Lưu theo revision hiện tại; nếu dữ liệu đã đổi, tải lại rồi thử
              lại.
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => void saveOrganization(event)}
            className="space-y-3"
          >
            <div className="space-y-1">
              <Label htmlFor="edit-platform-organization-name">
                Tên organization
              </Label>
              <Input
                id="edit-platform-organization-name"
                value={editingOrganizationName}
                onChange={(event) =>
                  setEditingOrganizationName(event.target.value)
                }
                required
                maxLength={120}
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="edit-platform-organization-limit">
                Giới hạn workspace
              </Label>
              <Input
                id="edit-platform-organization-limit"
                type="number"
                min={1}
                max={10000}
                value={editingOrganizationLimit}
                onChange={(event) =>
                  setEditingOrganizationLimit(Number(event.target.value))
                }
                required
              />
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => setEditingOrganization(null)}
                disabled={isSavingOrganization}
              >
                Hủy
              </Button>
              <Button
                type="submit"
                disabled={
                  !editingOrganizationName.trim() || isSavingOrganization
                }
              >
                {isSavingOrganization ? "Đang lưu…" : "Lưu thay đổi"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={createAccountOpen} onOpenChange={setCreateAccountOpen}>
        <DialogContent className="max-w-md p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">
              Tạo tài khoản nền tảng
            </DialogTitle>
            <DialogDescription className="text-xs">
              Tạo identity đang chờ xác minh; thao tác này không tạo workspace
              membership. Cấp quyền tại workspace đích riêng.
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => void createAccount(event)}
            className="space-y-3"
          >
            <div className="space-y-1">
              <Label htmlFor="platform-account-username">Username</Label>
              <Input
                id="platform-account-username"
                required
                minLength={3}
                maxLength={64}
                value={newUsername}
                onChange={(event) => setNewUsername(event.target.value)}
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="platform-account-email">Email</Label>
              <Input
                id="platform-account-email"
                type="email"
                required
                maxLength={254}
                value={newEmail}
                onChange={(event) => setNewEmail(event.target.value)}
              />
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => setCreateAccountOpen(false)}
                disabled={isCreatingAccount}
              >
                Hủy
              </Button>
              <Button type="submit" disabled={isCreatingAccount}>
                {isCreatingAccount ? "Đang tạo…" : "Tạo account"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={createResourceOpen} onOpenChange={setCreateResourceOpen}>
        <DialogContent className="max-w-md p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">Đăng ký tài nguyên</DialogTitle>
            <DialogDescription className="text-xs">
              Tài nguyên sẽ thuộc workspace được chọn.
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => void createResource(event)}
            className="space-y-3"
          >
            <div className="space-y-1">
              <Label htmlFor="resource-workspace">Workspace</Label>
              <Select
                value={resourceWorkspaceId}
                onValueChange={setResourceWorkspaceId}
              >
                <SelectTrigger id="resource-workspace">
                  <SelectValue placeholder="Chọn workspace" />
                </SelectTrigger>
                <SelectContent>
                  {workspaces.map(({ workspace, organization }) => (
                    <SelectItem key={workspace.id} value={workspace.id}>
                      {organization.name} / {workspace.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1">
              <Label htmlFor="resource-name">Tên tài nguyên</Label>
              <Input
                id="resource-name"
                required
                maxLength={160}
                value={resourceName}
                onChange={(event) => setResourceName(event.target.value)}
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="resource-kind">Loại tài nguyên</Label>
              <Select
                value={resourceKind}
                onValueChange={(value) =>
                  setResourceKind(value as ResourceKind)
                }
              >
                <SelectTrigger id="resource-kind">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="profile">Profile</SelectItem>
                  <SelectItem value="proxy">Proxy</SelectItem>
                  <SelectItem value="vpn">VPN</SelectItem>
                  <SelectItem value="cookieset">Cookie set</SelectItem>
                  <SelectItem value="extension">Extension</SelectItem>
                  <SelectItem value="runtime">Runtime</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => setCreateResourceOpen(false)}
                disabled={isCreatingResource}
              >
                Hủy
              </Button>
              <Button
                type="submit"
                disabled={!resourceWorkspaceId || isCreatingResource}
              >
                {isCreatingResource ? "Đang lưu…" : "Đăng ký"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog
        open={!!quotaWorkspace}
        onOpenChange={(open) => !open && setQuotaWorkspace(null)}
      >
        <DialogContent className="max-w-md p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">Hạn mức workspace</DialogTitle>
            <DialogDescription className="text-xs">
              {quotaWorkspace?.organization.name} /{" "}
              {quotaWorkspace?.workspace.name}
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => void saveQuota(event)}
            className="space-y-3"
          >
            <div className="space-y-1">
              <Label htmlFor="workspace-max-profiles">Giới hạn profile</Label>
              <Input
                id="workspace-max-profiles"
                type="number"
                min={0}
                step={1}
                required
                value={maxProfiles}
                onChange={(event) => setMaxProfiles(Number(event.target.value))}
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="workspace-max-members">Giới hạn thành viên</Label>
              <Input
                id="workspace-max-members"
                type="number"
                min={0}
                step={1}
                required
                value={maxMembers}
                onChange={(event) => setMaxMembers(Number(event.target.value))}
              />
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => setQuotaWorkspace(null)}
                disabled={isSavingQuota}
              >
                Hủy
              </Button>
              <Button
                type="submit"
                disabled={isSavingQuota || maxProfiles < 0 || maxMembers < 0}
              >
                {isSavingQuota ? "Đang lưu…" : "Lưu hạn mức"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog
        open={!!pendingAccount}
        onOpenChange={(open) => !open && setPendingAccount(null)}
      >
        <DialogContent className="max-w-sm p-4">
          <DialogHeader>
            <DialogTitle className="text-sm">
              Xác nhận thay đổi tài khoản
            </DialogTitle>
            <DialogDescription className="text-xs">
              Đổi trạng thái tài khoản này thành{" "}
              <strong className="text-foreground">
                {pendingAccount ? statusLabels[pendingAccount.status] : ""}
              </strong>
              ? Thao tác có hiệu lực trên tài khoản toàn cục.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPendingAccount(null)}
              disabled={isUpdatingAccount}
            >
              Hủy
            </Button>
            <Button
              size="sm"
              onClick={() => void updateAccountStatus()}
              disabled={isUpdatingAccount}
            >
              {isUpdatingAccount ? "Đang lưu…" : "Xác nhận"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

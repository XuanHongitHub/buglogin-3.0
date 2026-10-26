"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowUpDown,
  LuBuilding2,
  LuCheck,
  LuChevronLeft,
  LuChevronRight,
  LuHardDrive,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuSend,
  LuShieldCheck,
  LuTriangleAlert,
  LuUsers,
  LuWifiOff,
} from "react-icons/lu";
import { CreateInviteModal } from "@/components/create-invite-modal";
import { LoadingButton } from "@/components/loading-button";
import { MemberManagementDialog } from "@/components/member-management-dialog";
import { ScopedAccountCreationPage } from "@/components/scoped-account-creation-page";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { WorkspaceDetailPage } from "@/components/workspace-detail-page";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type {
  Organization,
  Workspace,
  WorkspacePolicy,
} from "../../frontend/src/generated/command-registry";
import { RippleButton } from "./ui/ripple";

interface WorkspaceManagementDialogProps {
  mode?: "page" | "dialog";
  isOpen?: boolean;
  onClose?: () => void;
  activeWorkspaceId: string;
  onWorkspaceSwitched: (workspaceId: string) => void;
  onWorkspaceDataChanged?: () => void;
  onNavigateHome?: () => void;
}

const desktopApi = createTauriDesktopApi();

export function WorkspaceManagementDialog({
  mode = "dialog",
  isOpen = true,
  onClose = () => {},
  activeWorkspaceId,
  onWorkspaceSwitched,
  onWorkspaceDataChanged,
  onNavigateHome,
}: WorkspaceManagementDialogProps) {
  const { t } = useTranslation();
  const isPageMode = mode === "page";
  const [organizations, setOrganizations] = useState<Organization[]>([]);
  const [isLoadingOrgs, setIsLoadingOrgs] = useState(true);
  const [organizationLoadError, setOrganizationLoadError] = useState<
    string | null
  >(null);
  const [selectedOrgId, setSelectedOrgId] = useState<string>("");
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [workspaceLoadError, setWorkspaceLoadError] = useState<string | null>(
    null,
  );
  const [searchQuery, setSearchQuery] = useState("");
  const [switchingId, setSwitchingId] = useState<string | null>(null);

  // Sub-modal states
  const [createWsOpen, setCreateWsOpen] = useState(false);
  const [createOrgOpen, setCreateOrgOpen] = useState(false);
  const [editingWs, setEditingWs] = useState<Workspace | null>(null);
  const [inviteWs, setInviteWs] = useState<Workspace | null>(null);
  const [membersWs, setMembersWs] = useState<Workspace | null>(null);
  const [accountWorkspace, setAccountWorkspace] = useState<Workspace | null>(
    null,
  );
  const [detailWorkspace, setDetailWorkspace] = useState<Workspace | null>(
    null,
  );
  const [detailInitialTab, setDetailInitialTab] = useState<
    "overview" | "members" | "groups" | "profiles"
  >("overview");
  const workspaceRequestRef = useRef(0);
  const organizationRequestRef = useRef(0);

  // Form states for Create Workspace
  const [wsName, setWsName] = useState("");
  const [wsMaxProfiles, setWsMaxProfiles] = useState(300);
  const [wsMaxMembers, setWsMaxMembers] = useState(10);
  const [wsOffline, setWsOffline] = useState(true);
  const [isSubmittingWs, setIsSubmittingWs] = useState(false);

  // Form states for Edit Workspace
  const [editWsName, setEditWsName] = useState("");
  const [editWsOrgId, setEditWsOrgId] = useState("");
  const [editWsMaxProfiles, setEditWsMaxProfiles] = useState(500);
  const [editWsMaxMembers, setEditWsMaxMembers] = useState(10);
  const [editWsOffline, setEditWsOffline] = useState(true);
  const [isUpdatingWs, setIsUpdatingWs] = useState(false);

  useEffect(() => {
    if (editingWs) {
      setEditWsName(editingWs.name);
      setEditWsOrgId(editingWs.organizationId);
      setEditWsMaxProfiles(editingWs.policy?.maxProfiles ?? 300);
      setEditWsMaxMembers(editingWs.policy?.maxMembers ?? 10);
      setEditWsOffline(editingWs.policy?.offlineEntitled ?? true);
    }
  }, [editingWs]);

  // Form states for Create Org
  const [orgName, setOrgName] = useState("");
  const [isSubmittingOrg, setIsSubmittingOrg] = useState(false);

  // Load organizations
  const loadOrgs = useCallback(async (preferredOrgId?: string) => {
    const requestId = ++organizationRequestRef.current;
    setIsLoadingOrgs(true);
    try {
      const orgPage = await desktopApi.listOrganizations();
      if (requestId !== organizationRequestRef.current) return orgPage.items;
      setOrganizations(orgPage.items);
      setOrganizationLoadError(null);
      setSelectedOrgId((current) =>
        preferredOrgId && orgPage.items.some((org) => org.id === preferredOrgId)
          ? preferredOrgId
          : orgPage.items.some((organization) => organization.id === current)
            ? current
            : (orgPage.items[0]?.id ?? ""),
      );
      return orgPage.items;
    } catch (err) {
      if (requestId !== organizationRequestRef.current) return null;
      console.error("Failed to load organizations:", err);
      setOrganizationLoadError(
        err instanceof Error ? err.message : String(err),
      );
      showErrorToast(String(err));
      return null;
    } finally {
      if (requestId === organizationRequestRef.current) setIsLoadingOrgs(false);
    }
  }, []);

  // Load workspaces for the selected organization
  const loadWorkspaces = useCallback(async () => {
    const requestId = ++workspaceRequestRef.current;
    setIsLoading(true);
    try {
      if (!selectedOrgId) {
        setWorkspaces([]);
        setWorkspaceLoadError(null);
        return;
      }
      const wsPage = await desktopApi.listWorkspaces(selectedOrgId);
      if (requestId !== workspaceRequestRef.current) return;
      setWorkspaces(wsPage.items);
      setWorkspaceLoadError(null);
    } catch (err) {
      if (requestId !== workspaceRequestRef.current) return;
      console.error("Failed to load workspaces:", err);
      setWorkspaces([]);
      setWorkspaceLoadError(err instanceof Error ? err.message : String(err));
      showErrorToast(
        t("workspaces.loadFailed", {
          defaultValue: "Không thể tải danh sách không gian làm việc",
        }),
      );
    } finally {
      if (requestId === workspaceRequestRef.current) setIsLoading(false);
    }
  }, [selectedOrgId, t]);

  useEffect(() => {
    if (isOpen) {
      void loadOrgs();
      void loadWorkspaces();
    }
  }, [isOpen, loadOrgs, loadWorkspaces]);

  const handleSwitchWorkspace = async (workspaceId: string) => {
    setSwitchingId(workspaceId);
    try {
      await invoke("select_active_workspace", { workspaceId });
      showSuccessToast(
        t("workspaces.switched", {
          defaultValue: "Đã chuyển sang không gian làm việc mới",
        }),
      );
      onWorkspaceSwitched(workspaceId);
      return true;
    } catch (err) {
      console.error("Failed to switch workspace:", err);
      showErrorToast(String(err));
      return false;
    } finally {
      setSwitchingId(null);
    }
  };

  const handleCreateWorkspace = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!wsName.trim() || !selectedOrgId) return;

    setIsSubmittingWs(true);
    try {
      const policy: WorkspacePolicy = {
        maxProfiles: Number(wsMaxProfiles) || 300,
        maxMembers: Number(wsMaxMembers) || 10,
        maxStorageBytes: 10 * 1024 * 1024 * 1024,
        offlineEntitled: wsOffline,
      };

      await desktopApi.createWorkspace({
        organizationId: selectedOrgId,
        workspace: {
          id: `ws_${crypto.randomUUID()}`,
          organizationId: selectedOrgId,
          name: wsName.trim(),
          policy,
          createdAt: new Date().toISOString(),
        },
      });

      showSuccessToast(
        t("workspaces.created", {
          defaultValue: `Đã tạo workspace "${wsName.trim()}" thành công`,
        }),
      );
      setWsName("");
      setCreateWsOpen(false);
      await loadWorkspaces();
      onWorkspaceDataChanged?.();
    } catch (err) {
      console.error("Failed to create workspace:", err);
      showErrorToast(String(err));
    } finally {
      setIsSubmittingWs(false);
    }
  };

  const handleCreateOrg = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!orgName.trim()) return;

    setIsSubmittingOrg(true);
    try {
      const orgId = `org_${crypto.randomUUID()}`;
      const account = await desktopApi.getAccount();
      const createdOrganization = await desktopApi.createOrganization({
        organization: {
          id: orgId,
          accountId: account.accountId,
          name: orgName.trim(),
          createdAt: new Date().toISOString(),
        },
        policy: {
          maxWorkspaces: 10,
          offlineEntitled: false,
        },
      });

      showSuccessToast(
        t("workspaces.orgCreated", {
          defaultValue: `Đã tạo tổ chức "${orgName.trim()}"`,
        }),
      );
      setOrgName("");
      setCreateOrgOpen(false);
      workspaceRequestRef.current += 1;
      setWorkspaces([]);
      setWorkspaceLoadError(null);
      setIsLoading(true);
      const refreshedOrganizations = await loadOrgs(orgId);
      setOrganizations((current) =>
        current.some((organization) => organization.id === orgId)
          ? current
          : [...current, createdOrganization],
      );
      if (
        !refreshedOrganizations?.some(
          (organization) => organization.id === orgId,
        )
      ) {
        setSelectedOrgId(orgId);
      }
      onWorkspaceDataChanged?.();
    } catch (err) {
      console.error("Failed to create organization:", err);
      showErrorToast(String(err));
    } finally {
      setIsSubmittingOrg(false);
    }
  };

  const handleUpdateWorkspace = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editingWs || !editWsName.trim()) return;

    setIsUpdatingWs(true);
    try {
      await desktopApi.updateWorkspace({
        organizationId: editingWs.organizationId,
        workspaceId: editingWs.id,
        update: {
          name: editWsName.trim(),
          policy: {
            maxProfiles:
              Number(editWsMaxProfiles) || editingWs.policy?.maxProfiles || 300,
            maxMembers:
              Number(editWsMaxMembers) || editingWs.policy?.maxMembers || 10,
            maxStorageBytes:
              editingWs.policy?.maxStorageBytes ?? 10 * 1024 * 1024 * 1024,
            offlineEntitled: editWsOffline,
          },
        },
      });

      showSuccessToast(
        t("workspaces.updated", {
          defaultValue: `Đã cập nhật workspace "${editWsName.trim()}"`,
        }),
      );
      setEditingWs(null);
      await loadWorkspaces();
      onWorkspaceDataChanged?.();
    } catch (err) {
      console.error("Failed to update workspace:", err);
      showErrorToast(String(err));
    } finally {
      setIsUpdatingWs(false);
    }
  };

  const [filterType, setFilterType] = useState<"all" | "active" | "hasMembers">(
    "all",
  );
  const [sortBy, setSortBy] = useState<
    "active" | "name" | "profiles" | "members"
  >("active");
  const [page, setPage] = useState(0);
  const pageSize = isPageMode ? 10 : 6;

  const processedWorkspaces = useMemo(() => {
    let list = workspaces;
    if (filterType === "active") {
      list = list.filter((w) => w.id === activeWorkspaceId);
    } else if (filterType === "hasMembers") {
      list = list.filter((w) => (w.policy?.maxMembers ?? 0) > 0);
    }
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      list = list.filter((w) => w.name.toLowerCase().includes(q));
    }
    return [...list].sort((a, b) => {
      if (sortBy === "active") {
        if (a.id === activeWorkspaceId) return -1;
        if (b.id === activeWorkspaceId) return 1;
        return a.name.localeCompare(b.name);
      }
      if (sortBy === "name") {
        return a.name.localeCompare(b.name);
      }
      if (sortBy === "profiles") {
        return (b.policy?.maxProfiles ?? 0) - (a.policy?.maxProfiles ?? 0);
      }
      if (sortBy === "members") {
        return (b.policy?.maxMembers ?? 0) - (a.policy?.maxMembers ?? 0);
      }
      return 0;
    });
  }, [workspaces, searchQuery, filterType, sortBy, activeWorkspaceId]);

  const totalPages = Math.max(
    1,
    Math.ceil(processedWorkspaces.length / pageSize),
  );
  const pagedWorkspaces = useMemo(() => {
    const start = page * pageSize;
    return processedWorkspaces.slice(start, start + pageSize);
  }, [processedWorkspaces, page, pageSize]);

  const cycleSort = () => {
    setSortBy((curr) => {
      if (curr === "active") return "name";
      if (curr === "name") return "profiles";
      if (curr === "profiles") return "members";
      return "active";
    });
    setPage(0);
  };

  const renderActiveWorkspaceInfo = () => {
    const activeWsDetail =
      workspaces.find((w) => w.id === activeWorkspaceId) || workspaces[0];
    if (!activeWsDetail) return null;
    return (
      <div className="rounded-lg border border-border/70 bg-muted/15 p-3 space-y-2 text-xs">
        <div className="flex items-center justify-between font-semibold text-foreground text-xs">
          <div className="flex items-center gap-2">
            <LuBuilding2 className="size-4 text-primary" />
            <span>THÔNG TIN CHI TIẾT WORKSPACE: "{activeWsDetail.name}"</span>
          </div>
          <Badge
            variant="outline"
            className="font-mono text-[10px] text-emerald-600 bg-emerald-500/10 border-emerald-500/30"
          >
            Active Context
          </Badge>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-2 text-[11px] text-muted-foreground pt-1 border-t border-border/40">
          <div>
            • <strong>Trạng thái:</strong>{" "}
            <span className="text-foreground/80 font-medium">
              Không gian làm việc chính (Đang hoạt động)
            </span>
          </div>
          <div>
            • <strong>Giới hạn lưu trữ:</strong>{" "}
            <span className="text-foreground/80 font-medium">
              {activeWsDetail.policy?.maxStorageBytes
                ? `${Math.round(activeWsDetail.policy.maxStorageBytes / (1024 * 1024))} MB`
                : "Không giới hạn dung lượng"}
            </span>
          </div>
          <div>
            • <strong>Chính sách Profile:</strong>{" "}
            <span className="text-foreground/80 font-medium">
              {activeWsDetail.policy?.maxProfiles != null
                ? `${activeWsDetail.policy.maxProfiles} Profiles`
                : "Không giới hạn"}
            </span>
            {" • "}
            <strong>Thành viên:</strong>{" "}
            <span className="text-foreground/80 font-medium">
              {activeWsDetail.policy?.maxMembers != null
                ? `${activeWsDetail.policy.maxMembers} Thành viên`
                : "Không giới hạn"}
            </span>
          </div>
          <div>
            • <strong>Hỗ trợ ngoại tuyến (Offline):</strong>{" "}
            {activeWsDetail.policy?.offlineEntitled
              ? "Được phép mở khi mất mạng"
              : "Chỉ Online"}
          </div>
        </div>
      </div>
    );
  };

  const renderTableContent = (isPageMode: boolean) => (
    <div className="space-y-4">
      {/* 4 Metric KPI Cards in Page Mode */}
      {isPageMode && (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
          <div className="rounded-xl border border-border/60 bg-card p-3.5 shadow-xs">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="text-xs font-medium">Tổng Workspace</span>
              <LuBuilding2 className="size-4 text-primary" />
            </div>
            <div className="mt-2 text-2xl font-bold text-foreground">
              {workspaces.length}
            </div>
            <div className="text-[11px] text-muted-foreground mt-0.5">
              Không gian làm việc
            </div>
          </div>
          <div className="rounded-xl border border-border/60 bg-card p-3.5 shadow-xs">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="text-xs font-medium">Đang Hoạt Động</span>
              <LuCheck className="size-4 text-emerald-500" />
            </div>
            <div className="mt-2 text-2xl font-bold text-emerald-600 truncate">
              {workspaces.find((w) => w.id === activeWorkspaceId)?.name || "—"}
            </div>
            <div className="text-[11px] text-muted-foreground mt-0.5">
              Phiên làm việc hiện tại
            </div>
          </div>
          <div className="rounded-xl border border-border/60 bg-card p-3.5 shadow-xs">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="text-xs font-medium">Tổng Hạn Mức Profile</span>
              <LuHardDrive className="size-4 text-primary" />
            </div>
            <div className="mt-2 text-2xl font-bold text-foreground">
              {workspaces.reduce(
                (sum, w) => sum + (w.policy?.maxProfiles || 0),
                0,
              ) || "—"}
            </div>
            <div className="text-[11px] text-muted-foreground mt-0.5">
              Hạn mức các Workspace
            </div>
          </div>
          <div className="rounded-xl border border-border/60 bg-card p-3.5 shadow-xs">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="text-xs font-medium">Hỗ Trợ Ngoại Tuyến</span>
              <LuShieldCheck className="size-4 text-emerald-500" />
            </div>
            <div className="mt-2 text-2xl font-bold text-foreground">
              {workspaces.filter((w) => w.policy?.offlineEntitled).length}/
              {workspaces.length}
            </div>
            <div className="text-[11px] text-muted-foreground mt-0.5">
              Workspace bật Offline
            </div>
          </div>
        </div>
      )}

      {/* Main Table Container */}
      <div
        className={cn(
          "rounded-lg border border-border overflow-hidden",
          isPageMode && "shadow-xs bg-card",
        )}
      >
        {/* Top Toolbar */}
        <div className="flex flex-wrap items-center justify-between gap-2.5 p-3 border-b border-border/50 bg-muted/20">
          {!isPageMode && (
            <div className="flex items-center gap-2">
              <span className="text-muted-foreground font-medium text-xs">
                Tổ chức:
              </span>
              {organizations.length > 0 ? (
                <Select
                  value={selectedOrgId}
                  onValueChange={(organizationId) => {
                    workspaceRequestRef.current += 1;
                    setWorkspaces([]);
                    setWorkspaceLoadError(null);
                    setSelectedOrgId(organizationId);
                    setPage(0);
                  }}
                >
                  <SelectTrigger className="h-8 w-52 text-xs">
                    <SelectValue placeholder="Chọn tổ chức..." />
                  </SelectTrigger>
                  <SelectContent>
                    {organizations.map((org) => (
                      <SelectItem
                        key={org.id}
                        value={org.id}
                        className="text-xs"
                      >
                        {org.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <Badge variant="outline" className="text-xs py-1">
                  Cá nhân & Nhóm mặc định
                </Badge>
              )}
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCreateOrgOpen(true)}
                className="h-8 text-xs gap-1 cursor-pointer"
              >
                <LuPlus className="size-3" />
                <span>
                  {t("workspaces.addOrg", { defaultValue: "Thêm Org" })}
                </span>
              </Button>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            {/* Filter Pills */}
            <div className="flex items-center gap-1">
              {(["all", "active", "hasMembers"] as const).map((tFilter) => {
                const label =
                  tFilter === "all"
                    ? `Tất cả (${workspaces.length})`
                    : tFilter === "active"
                      ? "Đang chọn"
                      : "Có thành viên";
                const isSelected = filterType === tFilter;
                return (
                  <button
                    key={tFilter}
                    type="button"
                    onClick={() => {
                      setFilterType(tFilter);
                      setPage(0);
                    }}
                    className={cn(
                      "px-2.5 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer",
                      isSelected
                        ? "bg-primary text-primary-foreground font-semibold shadow-2xs"
                        : "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
                    )}
                  >
                    {label}
                  </button>
                );
              })}
            </div>

            {/* Sort Toggle */}
            <Button
              variant="outline"
              size="sm"
              onClick={cycleSort}
              className="h-8 text-xs gap-1 cursor-pointer"
              title="Thay đổi thứ tự sắp xếp"
            >
              <LuArrowUpDown className="size-3 text-primary" />
              <span>
                {sortBy === "active"
                  ? "Sắp xếp: Đang dùng"
                  : sortBy === "name"
                    ? "Sắp xếp: Tên A-Z"
                    : sortBy === "profiles"
                      ? "Sắp xếp: Hạn mức"
                      : "Sắp xếp: Thành viên"}
              </span>
            </Button>

            <div className="relative w-44 sm:w-52">
              <LuSearch className="absolute left-2.5 top-2.5 size-3.5 text-muted-foreground" />
              <Input
                placeholder={t("workspaces.searchPlaceholder", {
                  defaultValue: "Tìm tên workspace...",
                })}
                value={searchQuery}
                onChange={(e) => {
                  setSearchQuery(e.target.value);
                  setPage(0);
                }}
                className="h-8 pl-8 text-xs"
              />
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => {
                void loadOrgs();
                void loadWorkspaces();
              }}
              disabled={isLoading || !selectedOrgId}
              className="h-8 gap-1.5 text-xs"
            >
              <LuRefreshCw
                className={cn("size-3", isLoading && "animate-spin")}
              />
              Làm mới
            </Button>
            <Button
              size="sm"
              onClick={() => setCreateWsOpen(true)}
              className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
            >
              <LuPlus className="size-3.5" />
              <span>
                {t("workspaces.createBtn", {
                  defaultValue: "Tạo Workspace Mới",
                })}
              </span>
            </Button>
          </div>
        </div>

        {/* Workspaces Table */}
        {workspaceLoadError ? (
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
            <span>Không tải được workspace. {workspaceLoadError}</span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void loadWorkspaces()}
              disabled={isLoading}
              className="h-7 text-xs"
            >
              Thử lại
            </Button>
          </div>
        ) : null}
        <div
          className={cn(
            "overflow-x-auto",
            isPageMode ? "" : "max-h-[300px] overflow-y-auto",
          )}
        >
          <table className="w-full min-w-[1140px] text-left text-xs">
            <thead className="bg-muted/50 border-b border-border sticky top-0 font-medium text-muted-foreground text-[11px]">
              <tr>
                <th className="py-2.5 px-3">Tên Không Gian Làm Việc</th>
                <th className="py-2.5 px-3 w-28">Hạn mức Profile</th>
                <th className="py-2.5 px-3 w-24">Thành viên</th>
                <th className="py-2.5 px-3 w-24">Ngoại tuyến</th>
                <th className="py-2.5 px-3 w-24 text-center">Trạng thái</th>
                <th className="w-[560px] min-w-[560px] px-3 py-2.5 text-center">
                  Thao tác
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {isLoading ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-8 text-center text-muted-foreground"
                  >
                    {t("common.buttons.loading", {
                      defaultValue: "Đang tải dữ liệu...",
                    })}
                  </td>
                </tr>
              ) : processedWorkspaces.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-8 text-center text-muted-foreground"
                  >
                    {t("workspaces.noWorkspacesFound", {
                      defaultValue:
                        workspaces.length === 0 && !searchQuery
                          ? "Organization này chưa có workspace."
                          : "Không tìm thấy workspace nào phù hợp với bộ lọc",
                    })}
                  </td>
                </tr>
              ) : (
                pagedWorkspaces.map((ws) => {
                  const isActive = ws.id === activeWorkspaceId;
                  const isSwitching = switchingId === ws.id;

                  return (
                    <tr
                      key={ws.id}
                      className={cn(
                        "transition-colors hover:bg-muted/30",
                        isActive && "bg-primary/5",
                      )}
                    >
                      <td className="px-3 py-2">
                        <div className="flex min-w-0 items-center gap-2.5">
                          <span className="grid size-7 shrink-0 place-items-center rounded bg-primary/10 text-primary font-semibold">
                            <LuBuilding2 className="size-3.5" />
                          </span>
                          <div className="min-w-0">
                            <div className="flex min-w-0 items-center gap-1.5 font-semibold text-foreground">
                              <span className="truncate" title={ws.name}>
                                {ws.name}
                              </span>
                              {isActive && (
                                <Badge
                                  variant="outline"
                                  className="h-4 border-emerald-500/30 bg-emerald-500/10 px-1.5 text-[9px] text-emerald-500 font-semibold"
                                >
                                  Đang chọn
                                </Badge>
                              )}
                            </div>
                            <div className="text-[10px] text-muted-foreground">
                              {organizations.find(
                                (o) => o.id === ws.organizationId,
                              )?.name || "Không gian làm việc"}
                            </div>
                          </div>
                        </div>
                      </td>
                      <td className="px-3 py-2 font-mono text-muted-foreground">
                        {ws.policy?.maxProfiles
                          ? `${ws.policy.maxProfiles} slots`
                          : "Không giới hạn"}
                      </td>
                      <td className="px-3 py-2 font-mono text-muted-foreground">
                        {ws.policy?.maxMembers
                          ? `${ws.policy.maxMembers} seats`
                          : "Không giới hạn"}
                      </td>
                      <td className="px-3 py-2">
                        {ws.policy?.offlineEntitled ? (
                          <span className="inline-flex items-center gap-1 text-[11px] text-emerald-600 font-medium">
                            <LuCheck className="size-3" />
                            <span>Được phép</span>
                          </span>
                        ) : (
                          <span className="inline-flex items-center gap-1 text-[11px] text-muted-foreground">
                            <LuWifiOff className="size-3" />
                            <span>Chỉ Online</span>
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-2 text-center">
                        {isActive ? (
                          <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-emerald-500">
                            <span className="size-1.5 rounded-full bg-emerald-500 animate-pulse" />
                            Active
                          </span>
                        ) : (
                          <LoadingButton
                            variant="outline"
                            size="sm"
                            isLoading={isSwitching}
                            onClick={() => void handleSwitchWorkspace(ws.id)}
                            className="h-7 px-2.5 text-xs cursor-pointer"
                          >
                            <span>Chuyển sang</span>
                          </LoadingButton>
                        )}
                      </td>
                      <td className="w-[560px] min-w-[560px] px-3 py-2 text-center">
                        <div className="flex flex-nowrap items-center justify-center gap-1 whitespace-nowrap">
                          {isPageMode && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => {
                                setDetailInitialTab("overview");
                                setDetailWorkspace(ws);
                              }}
                              className="h-7 px-2 text-xs"
                              title="Mở chi tiết workspace"
                            >
                              Mở workspace
                            </Button>
                          )}
                          {isPageMode && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => {
                                setDetailInitialTab("groups");
                                setDetailWorkspace(ws);
                              }}
                              className="h-7 px-2 text-xs"
                              title="Mở nhóm trong workspace này"
                            >
                              Mở groups
                            </Button>
                          )}
                          {isPageMode && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => setAccountWorkspace(ws)}
                              className="h-7 px-2 text-xs"
                            >
                              Tạo account
                            </Button>
                          )}
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => setEditingWs(ws)}
                            className="h-7 w-7 p-0 cursor-pointer"
                            title="Chỉnh sửa cài đặt workspace"
                          >
                            <LuPencil className="size-3 text-muted-foreground hover:text-foreground" />
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              if (isPageMode) {
                                setDetailInitialTab("members");
                                setDetailWorkspace(ws);
                              } else {
                                setMembersWs(ws);
                              }
                            }}
                            className="h-7 gap-1 px-2 text-xs cursor-pointer"
                            title="Quản lý thành viên & phân quyền"
                          >
                            <LuUsers className="size-3" />
                            <span>Thành viên</span>
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => setInviteWs(ws)}
                            className="h-7 w-7 p-0 cursor-pointer"
                            title="Tạo mã mời thành viên"
                          >
                            <LuSend className="size-3 text-muted-foreground hover:text-foreground" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>

        {/* Pagination Controls */}
        <div className="flex items-center justify-between border-t border-border/60 bg-muted/20 px-3 py-2 text-xs text-muted-foreground">
          <span>
            Hiển thị {processedWorkspaces.length > 0 ? page * pageSize + 1 : 0}-
            {Math.min((page + 1) * pageSize, processedWorkspaces.length)} trên{" "}
            {processedWorkspaces.length} workspace
          </span>
          <div className="flex items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              disabled={page === 0}
              onClick={() => setPage((p) => p - 1)}
              className="h-7 px-2 text-xs gap-1 cursor-pointer"
            >
              <LuChevronLeft className="size-3.5" />
              <span>Trước</span>
            </Button>
            <span className="font-mono text-xs px-1.5 font-medium text-foreground">
              {page + 1} / {totalPages}
            </span>
            <Button
              variant="outline"
              size="sm"
              disabled={page >= totalPages - 1}
              onClick={() => setPage((p) => p + 1)}
              className="h-7 px-2 text-xs gap-1 cursor-pointer"
            >
              <span>Sau</span>
              <LuChevronRight className="size-3.5" />
            </Button>
          </div>
        </div>
      </div>

      {renderActiveWorkspaceInfo()}
    </div>
  );

  const renderSubModals = () => (
    <>
      {/* SUB-MODAL: Create Workspace */}
      <Dialog open={createWsOpen} onOpenChange={setCreateWsOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold">
              {t("workspaces.createModalTitle", {
                defaultValue: "Khởi Tạo Không Gian Làm Việc Mới",
              })}
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              Tạo không gian cô lập an toàn để lưu trữ Profile và chia sẻ theo
              nhóm
            </DialogDescription>
          </DialogHeader>

          <form
            onSubmit={(e) => void handleCreateWorkspace(e)}
            className="space-y-4 py-2 text-xs"
          >
            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                Tên Không Gian Làm Việc (Workspace Name)
              </Label>
              <Input
                placeholder="VD: Shopee Ads Global Q4"
                value={wsName}
                onChange={(e) => setWsName(e.target.value)}
                required
                className="h-9 text-xs"
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">Hạn mức Profiles</Label>
                <Input
                  type="number"
                  min={1}
                  max={10000}
                  value={wsMaxProfiles}
                  onChange={(e) => setWsMaxProfiles(Number(e.target.value))}
                  className="h-9 text-xs"
                />
              </div>
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  Hạn mức Thành viên
                </Label>
                <Input
                  type="number"
                  min={1}
                  max={500}
                  value={wsMaxMembers}
                  onChange={(e) => setWsMaxMembers(Number(e.target.value))}
                  className="h-9 text-xs"
                />
              </div>
            </div>

            <div className="flex items-center gap-2 pt-1 text-xs text-muted-foreground cursor-pointer select-none">
              <Checkbox
                aria-label="Cho phép mở Profile ngoại tuyến khi mất mạng"
                checked={wsOffline}
                onCheckedChange={(checked) => setWsOffline(checked === true)}
              />
              <span>
                Cho phép mở Profile ngoại tuyến khi mất mạng (Offline Entitled)
              </span>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setCreateWsOpen(false)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy
              </Button>
              <LoadingButton
                type="submit"
                size="sm"
                isLoading={isSubmittingWs}
                disabled={!wsName.trim()}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Khởi Tạo Workspace
              </LoadingButton>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Create Organization */}
      <Dialog open={createOrgOpen} onOpenChange={setCreateOrgOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold">
              {t("workspaces.createOrgTitle", {
                defaultValue: "Tạo Tổ Chức (Organization) Mới",
              })}
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              Tổ chức chứa nhiều không gian làm việc và quản lý gói tài nguyên
              tổng thể
            </DialogDescription>
          </DialogHeader>

          <form
            onSubmit={(e) => void handleCreateOrg(e)}
            className="space-y-4 py-2 text-xs"
          >
            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                Tên Tổ Chức (Organization Name)
              </Label>
              <Input
                placeholder="VD: Alpha Media Agency"
                value={orgName}
                onChange={(e) => setOrgName(e.target.value)}
                required
                className="h-9 text-xs"
              />
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setCreateOrgOpen(false)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy
              </Button>
              <LoadingButton
                type="submit"
                size="sm"
                isLoading={isSubmittingOrg}
                disabled={!orgName.trim()}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Tạo Tổ Chức
              </LoadingButton>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Edit Workspace (Section 14.2) */}
      <Dialog
        open={!!editingWs}
        onOpenChange={(open) => !open && setEditingWs(null)}
      >
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold">
              {t("workspaces.editTitle", {
                defaultValue: "Cài đặt không gian làm việc",
              })}
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {t("workspaces.editDesc", {
                defaultValue:
                  "Cập nhật tên, chính sách và hạn mức phân bổ cho không gian này",
              })}
            </DialogDescription>
          </DialogHeader>

          {editingWs && (
            <form
              onSubmit={(e) => void handleUpdateWorkspace(e)}
              className="space-y-4 py-2 text-xs"
            >
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">Tên Workspace: *</Label>
                <Input
                  value={editWsName}
                  onChange={(e) => setEditWsName(e.target.value)}
                  required
                  className="h-9 text-xs"
                />
              </div>

              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  Thuộc Tổ Chức (Parent Organization):
                </Label>
                <Select value={editWsOrgId} onValueChange={setEditWsOrgId}>
                  <SelectTrigger className="h-9 text-xs">
                    <SelectValue placeholder="Chọn tổ chức..." />
                  </SelectTrigger>
                  <SelectContent>
                    {organizations.map((org) => (
                      <SelectItem
                        key={org.id}
                        value={org.id}
                        className="text-xs"
                      >
                        <span className="inline-flex items-center gap-1.5">
                          <LuBuilding2 className="size-3.5 text-muted-foreground" />
                          {org.name} ({org.id.slice(0, 12)}...)
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label className="text-xs font-medium">
                    Hạn mức Profiles
                  </Label>
                  <Input
                    type="number"
                    min={1}
                    max={10000}
                    value={editWsMaxProfiles}
                    onChange={(e) =>
                      setEditWsMaxProfiles(Number(e.target.value))
                    }
                    className="h-9 text-xs"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label className="text-xs font-medium">
                    Hạn mức Thành viên
                  </Label>
                  <Input
                    type="number"
                    min={1}
                    max={500}
                    value={editWsMaxMembers}
                    onChange={(e) =>
                      setEditWsMaxMembers(Number(e.target.value))
                    }
                    className="h-9 text-xs"
                  />
                </div>
              </div>

              <div className="flex items-center gap-2 pt-1 text-xs text-muted-foreground cursor-pointer select-none">
                <Checkbox
                  aria-label="Cho phép mở Profile ngoại tuyến khi mất mạng"
                  checked={editWsOffline}
                  onCheckedChange={(checked) =>
                    setEditWsOffline(checked === true)
                  }
                />
                <span>
                  Cho phép mở Profile ngoại tuyến khi mất mạng (Offline
                  Entitled)
                </span>
              </div>

              <div className="rounded-lg border border-border/60 p-3 space-y-1.5">
                <div className="font-medium text-xs flex items-center gap-1.5">
                  <LuTriangleAlert className="size-3.5 text-muted-foreground" />
                  <span>Xóa workspace</span>
                </div>
                <p className="text-[11px] text-muted-foreground">
                  API chưa hỗ trợ xóa hoặc lưu trữ workspace. Đổi tên không xóa
                  dữ liệu nên thao tác này chưa được cung cấp.
                </p>
              </div>

              <DialogFooter className="border-t border-border/50 pt-3">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setEditingWs(null)}
                  className="h-8 text-xs cursor-pointer"
                >
                  Hủy Thay Đổi
                </Button>
                <LoadingButton
                  type="submit"
                  size="sm"
                  isLoading={isUpdatingWs}
                  disabled={!editWsName.trim()}
                  className="h-8 text-xs font-medium cursor-pointer"
                >
                  Lưu Cập Nhật
                </LoadingButton>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Create Invite Modal (Section 14.1) */}
      {inviteWs && (
        <CreateInviteModal
          isOpen={!!inviteWs}
          onClose={() => setInviteWs(null)}
          workspaceId={inviteWs.id}
          workspaceName={inviteWs.name}
        />
      )}

      {/* SUB-MODAL: Member Management Dialog (Section 4.2) */}
      {membersWs && (
        <MemberManagementDialog
          isOpen={!!membersWs}
          onClose={() => setMembersWs(null)}
          workspaceId={membersWs.id}
          workspaceName={membersWs.name}
        />
      )}
    </>
  );

  if (mode === "page" && detailWorkspace) {
    return (
      <WorkspaceDetailPage
        organizationId={detailWorkspace.organizationId}
        organizationName={
          organizations.find((org) => org.id === detailWorkspace.organizationId)
            ?.name ?? "Organization"
        }
        workspaceId={detailWorkspace.id}
        workspaceName={detailWorkspace.name}
        initialTab={detailInitialTab}
        onBack={() => setDetailWorkspace(null)}
      />
    );
  }

  if (mode === "page" && membersWs) {
    return (
      <MemberManagementDialog
        mode="page"
        isOpen={true}
        onClose={() => setMembersWs(null)}
        workspaceId={membersWs.id}
        workspaceName={membersWs.name}
        organizationName={
          organizations.find((org) => org.id === membersWs.organizationId)?.name
        }
      />
    );
  }

  if (mode === "page" && accountWorkspace) {
    return (
      <ScopedAccountCreationPage
        workspaceId={accountWorkspace.id}
        workspaceName={accountWorkspace.name}
        organizationName={
          organizations.find(
            (org) => org.id === accountWorkspace.organizationId,
          )?.name
        }
        onBack={() => setAccountWorkspace(null)}
        onCreated={() => void loadWorkspaces()}
      />
    );
  }

  if (mode === "page") {
    return (
      <div className="flex flex-1 flex-col overflow-hidden bg-background">
        {/* Full-Page Header */}
        <div className="flex items-center justify-between border-b border-border/60 px-6 py-3.5 bg-card/30">
          <div className="flex items-center gap-3">
            <div className="grid size-9 place-items-center rounded-lg bg-primary/10 text-primary shadow-xs">
              <LuBuilding2 className="size-5" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h1 className="text-base font-semibold text-foreground">
                  {t("workspaces.managementTitle", {
                    defaultValue:
                      "Quản Lý Không Gian Làm Việc (Workspace Center)",
                  })}
                </h1>
                <Badge
                  variant="outline"
                  className="text-[10px] font-mono border-primary/30 text-primary"
                >
                  {workspaces.length} Không gian
                </Badge>
              </div>
              <p className="text-xs text-muted-foreground mt-0.5">
                {t("workspaces.managementSubtitle", {
                  defaultValue:
                    "Phân chia môi trường vận hành riêng biệt giữa các dự án, khách hàng và phòng ban",
                })}
              </p>
            </div>
          </div>
          {onNavigateHome && (
            <Button
              variant="outline"
              size="sm"
              onClick={onNavigateHome}
              className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
            >
              <span>← Trở về Profiles</span>
            </Button>
          )}
        </div>

        {/* Scrollable Page Body */}
        <div className="grid min-h-0 flex-1 grid-cols-1 overflow-hidden sm:grid-cols-[minmax(190px,240px)_minmax(0,1fr)]">
          <aside className="flex max-h-48 min-h-0 flex-col border-b border-border/60 bg-muted/10 p-3 sm:max-h-none sm:border-b-0 sm:border-r">
            <div className="flex items-center justify-between gap-2 px-2 pb-2">
              <h2 className="text-xs font-semibold">Organizations</h2>
              <Badge variant="outline" className="text-[10px]">
                {isLoadingOrgs ? "…" : organizations.length}
              </Badge>
            </div>
            <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
              {organizationLoadError ? (
                <div className="space-y-2 px-2 py-3 text-xs text-destructive">
                  <p>Không tải được danh sách organization.</p>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void loadOrgs()}
                    disabled={isLoadingOrgs}
                    className="h-7 text-xs"
                  >
                    Thử lại
                  </Button>
                </div>
              ) : null}
              {isLoadingOrgs && organizations.length === 0 ? (
                <div
                  className="flex items-center gap-2 px-2 py-3 text-xs text-muted-foreground"
                  role="status"
                >
                  <LuRefreshCw className="size-3 animate-spin" />
                  Đang tải organizations...
                </div>
              ) : null}
              {organizations.map((organization) => (
                <Button
                  key={organization.id}
                  type="button"
                  variant={
                    selectedOrgId === organization.id ? "secondary" : "ghost"
                  }
                  className="h-auto min-h-9 w-full justify-start whitespace-normal px-2.5 py-2 text-left text-xs"
                  aria-pressed={selectedOrgId === organization.id}
                  onClick={() => {
                    workspaceRequestRef.current += 1;
                    setWorkspaces([]);
                    setIsLoading(true);
                    setWorkspaceLoadError(null);
                    setSelectedOrgId(organization.id);
                    setPage(0);
                  }}
                >
                  <LuBuilding2 className="mr-2 size-3.5 shrink-0" />
                  <span className="truncate">{organization.name}</span>
                </Button>
              ))}
              {organizations.length === 0 &&
                !isLoadingOrgs &&
                !organizationLoadError && (
                  <p className="px-2 py-3 text-xs text-muted-foreground">
                    Chưa có organization.
                  </p>
                )}
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setCreateOrgOpen(true)}
              className="mt-3 w-full gap-1.5 text-xs"
            >
              <LuPlus className="size-3.5" />
              Tạo organization
            </Button>
          </aside>
          <div className="min-h-0 overflow-y-auto px-4 py-4 md:px-6">
            {renderTableContent(true)}
          </div>
        </div>

        {renderSubModals()}
      </div>
    );
  }

  return (
    <>
      <Dialog open={isOpen} onOpenChange={onClose}>
        <DialogContent className="max-w-3xl p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
                  <LuBuilding2 className="size-5" />
                </div>
                <div>
                  <DialogTitle className="text-base font-semibold">
                    {t("workspaces.managementTitle", {
                      defaultValue: "Quản Lý Tổ Chức & Không Gian Làm Việc",
                    })}
                  </DialogTitle>
                  <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                    {t("workspaces.managementSubtitle", {
                      defaultValue:
                        "Phân chia môi trường vận hành riêng biệt giữa các dự án, khách hàng và phòng ban",
                    })}
                  </DialogDescription>
                </div>
              </div>
            </div>
          </DialogHeader>

          <div className="space-y-3.5 py-1 text-xs">
            {renderTableContent(false)}
          </div>

          <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
            <span className="text-xs text-muted-foreground">
              Tổng cộng:{" "}
              <strong className="text-foreground">{workspaces.length}</strong>{" "}
              Workspaces
            </span>
            <RippleButton
              variant="outline"
              size="sm"
              onClick={onClose}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              {t("common.buttons.close", { defaultValue: "Đóng" })}
            </RippleButton>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {renderSubModals()}
    </>
  );
}

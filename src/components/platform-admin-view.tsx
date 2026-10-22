"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuBuilding2,
  LuCpu,
  LuHardDrive,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuServer,
  LuShieldAlert,
  LuUsers,
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type { Workspace } from "../../apps/desktop/src/contracts";

const desktopApi = createTauriDesktopApi();

export interface TenantRecord {
  id: string;
  organizationId: string;
  policy: Workspace["policy"];
  name: string;
  ownerEmail: string;
  tier: "STARTER" | "PRO TEAM" | "ENTERPRISE" | "UNSPECIFIED";
  profilesUsed: number | null;
  profilesMax: number;
  membersUsed: number | null;
  membersMax: number;
  status: "unknown";
}

interface PlatformAdminViewProps {
  onNavigateHome: () => void;
  onOpenAuditLogs?: () => void;
}

export function PlatformAdminView({
  onNavigateHome,
  onOpenAuditLogs,
}: PlatformAdminViewProps) {
  const { t } = useTranslation();
  const [tenants, setTenants] = useState<TenantRecord[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [tierFilter, setTierFilter] = useState<string>("all");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [sortKey, setSortKey] = useState<keyof TenantRecord>("name");
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);

  // System Hardware Stats
  const [systemInfo, setSystemInfo] = useState<{
    os: string;
    appVersion: string;
    totalMemGb: number;
    cpuCores: number;
  }>({
    os: "Unavailable",
    appVersion: "Unavailable",
    totalMemGb: 0,
    cpuCores: 0,
  });

  // Quota adjust state
  const [adjustTenant, setAdjustTenant] = useState<TenantRecord | null>(null);
  const [newMaxProfiles, setNewMaxProfiles] = useState(0);

  // Multi-select for bulk tenant actions
  const [selectedTenantIds, setSelectedTenantIds] = useState<Set<string>>(
    new Set(),
  );
  const [isBulkExecuting, setIsBulkExecuting] = useState(false);

  // Load Real Data from Desktop / Backend
  const loadPlatformData = useCallback(async () => {
    setIsLoading(true);
    try {
      // 1. Use the registered system health command. The Donut-only
      // get_system_info invoke is not part of the BugLogin desktop contract.
      try {
        const health = await desktopApi.getSystemHealth();
        setSystemInfo((prev) => ({
          ...prev,
          appVersion: `${health.service} · ${health.status}`,
          os: navigator.platform || "Unavailable",
          cpuCores: navigator.hardwareConcurrency ?? 0,
        }));
      } catch (err) {
        console.warn("Could not query system telemetry:", err);
      }

      // 2. Workspaces must be listed under their real organization IDs.
      let loadedWorkspaces: Workspace[] = [];
      try {
        const organizations = await desktopApi.listOrganizations();
        const workspacePages = await Promise.all(
          organizations.items.map((organization) =>
            desktopApi.listWorkspaces(organization.id),
          ),
        );
        loadedWorkspaces = workspacePages.flatMap((page) => page.items ?? []);
      } catch (err) {
        console.warn("Could not load workspaces:", err);
      }

      // 3. Read usage using each workspace's authorized API scope.
      if (loadedWorkspaces.length > 0) {
        const mapped = await Promise.all(
          loadedWorkspaces.map(async (ws): Promise<TenantRecord> => {
            let profilesUsed: number | null = 0;
            let membersUsed: number | null = 0;
            try {
              let cursor: string | undefined;
              const seen = new Set<string>();
              let pages = 0;
              do {
                const page = await desktopApi.listProfiles(false, {
                  workspaceId: ws.id,
                  limit: 100,
                  ...(cursor ? { cursor } : {}),
                });
                profilesUsed += page.items.length;
                cursor = page.nextCursor ?? undefined;
                pages += 1;
                if (cursor && (seen.has(cursor) || pages >= 1000)) {
                  throw new Error("Profile usage pagination did not advance");
                }
                if (cursor) seen.add(cursor);
              } while (cursor);
            } catch (err) {
              console.warn(`Could not load profile usage for ${ws.id}:`, err);
              profilesUsed = null;
            }
            try {
              membersUsed = (await desktopApi.listMembers(ws.id)).items.length;
            } catch (err) {
              console.warn(`Could not load member usage for ${ws.id}:`, err);
              membersUsed = null;
            }
            return {
              id: ws.id,
              organizationId: ws.organizationId,
              policy: ws.policy,
              name: ws.name,
              ownerEmail: "—",
              tier: "UNSPECIFIED",
              profilesUsed,
              profilesMax: ws.policy.maxProfiles,
              membersUsed,
              membersMax: ws.policy.maxMembers,
              status: "unknown",
            };
          }),
        );
        setTenants(mapped);
      } else {
        setTenants([]);
      }
    } catch (err) {
      showErrorToast(`Lỗi đồng bộ dữ liệu quản trị: ${String(err)}`);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadPlatformData();
  }, [loadPlatformData]);

  // Sorting & Filtering
  const filteredTenants = useMemo(() => {
    let result = tenants.filter((tenant) => {
      if (tierFilter !== "all" && tenant.tier !== tierFilter) {
        return false;
      }
      if (statusFilter !== "all" && tenant.status !== statusFilter) {
        return false;
      }
      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        if (
          !tenant.name.toLowerCase().includes(q) &&
          !tenant.id.toLowerCase().includes(q) &&
          !tenant.ownerEmail.toLowerCase().includes(q)
        ) {
          return false;
        }
      }
      return true;
    });

    result = [...result].sort((a, b) => {
      const aVal = a[sortKey];
      const bVal = b[sortKey];
      if (typeof aVal === "number" && typeof bVal === "number") {
        return sortDirection === "asc" ? aVal - bVal : bVal - aVal;
      }
      return sortDirection === "asc"
        ? String(aVal).localeCompare(String(bVal))
        : String(bVal).localeCompare(String(aVal));
    });

    return result;
  }, [tenants, tierFilter, statusFilter, searchQuery, sortKey, sortDirection]);

  // Pagination
  const totalPages = Math.max(1, Math.ceil(filteredTenants.length / pageSize));
  const paginatedTenants = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredTenants.slice(start, start + pageSize);
  }, [filteredTenants, currentPage, pageSize]);

  const handleSort = (key: keyof TenantRecord) => {
    if (sortKey === key) {
      setSortDirection((prev) => (prev === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDirection("asc");
    }
    setCurrentPage(1);
  };

  const handleToggleSelectTenant = (id: string) => {
    setSelectedTenantIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSelectAllTenants = () => {
    if (
      selectedTenantIds.size === paginatedTenants.length &&
      paginatedTenants.length > 0
    ) {
      setSelectedTenantIds(new Set());
    } else {
      setSelectedTenantIds(new Set(paginatedTenants.map((t) => t.id)));
    }
  };

  const handleBulkAddQuota = async () => {
    if (selectedTenantIds.size === 0) return;
    setIsBulkExecuting(true);
    let updated = 0;
    try {
      const selected = tenants.filter((tenant) =>
        selectedTenantIds.has(tenant.id),
      );
      let failed = 0;
      for (const tenant of selected) {
        try {
          await desktopApi.updateWorkspace({
            organizationId: tenant.organizationId,
            workspaceId: tenant.id,
            update: {
              policy: {
                ...tenant.policy,
                maxProfiles: tenant.profilesMax + 500,
              },
            },
          });
          updated += 1;
        } catch (err) {
          failed += 1;
          console.warn(
            `Failed to update quota for workspace ${tenant.id}:`,
            err,
          );
        }
      }
      if (updated) await loadPlatformData();
      if (failed === 0)
        showSuccessToast(`Đã cập nhật quota cho ${updated} workspace.`);
      else
        showErrorToast(
          `Đã cập nhật ${updated}/${selected.length} workspace; ${failed} thao tác thất bại.`,
        );
      setSelectedTenantIds(new Set());
    } finally {
      setIsBulkExecuting(false);
    }
  };

  const handleOpenAdjustQuota = (tenant: TenantRecord) => {
    setAdjustTenant(tenant);
    setNewMaxProfiles(tenant.profilesMax);
  };

  const handleSaveQuota = async () => {
    if (!adjustTenant) return;
    try {
      await desktopApi.updateWorkspace({
        organizationId: adjustTenant.organizationId,
        workspaceId: adjustTenant.id,
        update: {
          policy: { ...adjustTenant.policy, maxProfiles: newMaxProfiles },
        },
      });
      showSuccessToast(
        `Đã điều chỉnh hạn mức profiles của ${adjustTenant.name} lên ${newMaxProfiles}`,
      );
      setAdjustTenant(null);
      await loadPlatformData();
    } catch (err) {
      showErrorToast(err instanceof Error ? err.message : JSON.stringify(err));
    }
  };

  // Aggregates
  const totalProfilesUsed = tenants.reduce(
    (acc, t) => acc + (t.profilesUsed ?? 0),
    0,
  );
  const totalProfilesMax = tenants.reduce((acc, t) => acc + t.profilesMax, 0);
  const totalMembersUsed = tenants.reduce(
    (acc, t) => acc + (t.membersUsed ?? 0),
    0,
  );
  const totalMembersMax = tenants.reduce((acc, t) => acc + t.membersMax, 0);
  const profileUsageComplete = tenants.every(
    (tenant) => tenant.profilesUsed !== null,
  );
  const memberUsageComplete = tenants.every(
    (tenant) => tenant.membersUsed !== null,
  );

  return (
    <div className="flex h-full flex-col min-h-0 bg-background overflow-hidden">
      {/* 1. Header Toolbar */}
      <div className="border-b border-border/60 bg-card/60 px-6 py-4 shrink-0 backdrop-blur-xs">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={onNavigateHome}
              className="grid size-9 place-items-center rounded-lg border border-border/70 bg-background text-muted-foreground hover:bg-muted hover:text-foreground transition-colors cursor-pointer"
              title="Quay lại danh sách profiles"
            >
              <LuArrowLeft className="size-4" />
            </button>
            <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
              <LuServer className="size-5" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h1 className="text-base font-semibold text-foreground">
                  {t("platformAdmin.title", {
                    defaultValue: "Bàn Điều Hành Quản Trị Nền Tảng",
                  })}
                </h1>
                <Badge
                  variant="outline"
                  className="text-[10px] font-mono border-primary/30 text-primary"
                >
                  ENTERPRISE CONTROL
                </Badge>
              </div>
              <p className="text-xs text-muted-foreground mt-0.5">
                {t("platformAdmin.subtitle", {
                  defaultValue:
                    "Giám sát hệ sinh thái Multi-Tenant, phân bổ Quota máy chủ, trạng thái phần cứng và cấp phép bản quyền BugLogin.",
                })}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => void loadPlatformData()}
              disabled={isLoading}
              className="h-8 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuRefreshCw
                className={cn("size-3.5", isLoading && "animate-spin")}
              />
              <span>Làm mới</span>
            </Button>
            {onOpenAuditLogs && (
              <Button
                variant="outline"
                size="sm"
                onClick={onOpenAuditLogs}
                className="h-8 text-xs gap-1.5 cursor-pointer font-medium"
              >
                <LuShieldAlert className="size-3.5 text-primary" />
                <span>Nhật ký Audit</span>
              </Button>
            )}
          </div>
        </div>
      </div>

      {/* 2. Platform Telemetry Metrics */}
      <div className="border-b border-border/50 bg-muted/20 px-6 py-3 shrink-0">
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
          {/* Card 1: Profiles Quota */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuHardDrive className="size-3.5 text-primary" />
                <span>Dung Lượng Profiles:</span>
              </span>
              <span className="font-bold text-foreground font-mono">
                {profileUsageComplete
                  ? totalProfilesUsed.toLocaleString()
                  : "—"}{" "}
                / {totalProfilesMax.toLocaleString()}
              </span>
            </div>
            <div className="h-1.5 w-full rounded-full bg-muted overflow-hidden">
              <div
                className="h-full bg-primary rounded-full transition-all duration-300"
                style={{
                  width: profileUsageComplete
                    ? `${Math.min(100, Math.round((totalProfilesUsed / Math.max(1, totalProfilesMax)) * 100))}%`
                    : "0%",
                }}
              />
            </div>
            <div className="text-[10px] text-muted-foreground flex justify-between">
              <span>Mức dùng theo workspace</span>
              <span className="font-semibold text-foreground">
                {profileUsageComplete
                  ? `${Math.round((totalProfilesUsed / Math.max(1, totalProfilesMax)) * 100)}%`
                  : "—"}
              </span>
            </div>
          </div>

          {/* Card 2: Team Members */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuUsers className="size-3.5 text-primary" />
                <span>Ghế Thành Viên (Seats):</span>
              </span>
              <span className="font-bold text-foreground font-mono">
                {memberUsageComplete ? totalMembersUsed : "—"} /{" "}
                {totalMembersMax}
              </span>
            </div>
            <div className="h-1.5 w-full rounded-full bg-muted overflow-hidden">
              <div
                className="h-full bg-emerald-500 rounded-full transition-all duration-300"
                style={{
                  width: memberUsageComplete
                    ? `${Math.min(100, Math.round((totalMembersUsed / Math.max(1, totalMembersMax)) * 100))}%`
                    : "0%",
                }}
              />
            </div>
            <div className="text-[10px] text-muted-foreground flex justify-between">
              <span>
                {memberUsageComplete
                  ? `Còn ${Math.max(0, totalMembersMax - totalMembersUsed)} ghế khả dụng`
                  : "Mức dùng chưa tải đầy đủ"}
              </span>
              <span className="font-semibold text-foreground">
                {memberUsageComplete
                  ? `${Math.round((totalMembersUsed / Math.max(1, totalMembersMax)) * 100)}%`
                  : "—"}
              </span>
            </div>
          </div>

          {/* Card 3: Active Tenancy */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuBuilding2 className="size-3.5 text-primary" />
                <span>Không Gian / Tổ Chức:</span>
              </span>
              <span className="font-bold text-foreground font-mono">
                {tenants.length} workspace
              </span>
            </div>
            <div className="text-[10px] text-muted-foreground flex justify-between">
              <span>Trạng thái workspace chưa thuộc API hiện tại</span>
              <span className="font-semibold text-muted-foreground">
                Chưa xác định
              </span>
            </div>
          </div>

          {/* Card 4: Hardware Kernel */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuCpu className="size-3.5 text-primary" />
                <span>Hạ Tầng Phần Cứng:</span>
              </span>
              <Badge
                variant="outline"
                className="text-[9px] font-mono py-0 h-4 bg-muted"
              >
                {systemInfo.os}
              </Badge>
            </div>
            <div className="text-xs font-semibold text-foreground font-mono truncate">
              {systemInfo.cpuCores || "—"} Cores ·{" "}
              {systemInfo.totalMemGb || "—"}GB RAM
            </div>
            <div className="text-[10px] text-muted-foreground flex justify-between">
              <span>{systemInfo.appVersion}</span>
              <span className="text-muted-foreground font-medium">
                Local device
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* 3. Main Enterprise Data Table Area */}
      <div className="flex-1 flex flex-col min-h-0 p-6 overflow-hidden">
        <div className="flex-1 flex flex-col min-h-0 rounded-xl border border-border/70 bg-card shadow-xs overflow-hidden">
          {/* Table Toolbar Controls */}
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/50 bg-muted/15 p-3">
            <div className="flex flex-1 items-center gap-2 max-w-md">
              <div className="relative w-full">
                <LuSearch className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
                <Input
                  type="search"
                  placeholder="Tìm theo tên tổ chức, ID hoặc email chủ sở hữu…"
                  value={searchQuery}
                  onChange={(e) => {
                    setSearchQuery(e.target.value);
                    setCurrentPage(1);
                  }}
                  className="h-8 pl-8 text-xs bg-background"
                />
              </div>
            </div>

            <div className="flex items-center gap-2">
              <Select
                value={tierFilter}
                onValueChange={(val) => {
                  setTierFilter(val);
                  setCurrentPage(1);
                }}
              >
                <SelectTrigger className="h-8 w-36 text-xs bg-background">
                  <SelectValue placeholder="Gói cước" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all" className="text-xs">
                    Tất cả gói
                  </SelectItem>
                  <SelectItem value="UNSPECIFIED" className="text-xs">
                    Chưa cấu hình gói
                  </SelectItem>
                </SelectContent>
              </Select>

              <Select
                value={statusFilter}
                onValueChange={(val) => {
                  setStatusFilter(val);
                  setCurrentPage(1);
                }}
              >
                <SelectTrigger className="h-8 w-36 text-xs bg-background">
                  <SelectValue placeholder="Trạng thái" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all" className="text-xs">
                    Tất cả trạng thái
                  </SelectItem>
                  <SelectItem value="unknown" className="text-xs">
                    Chưa xác định
                  </SelectItem>
                </SelectContent>
              </Select>

              <Select
                value={String(pageSize)}
                onValueChange={(val) => {
                  setPageSize(Number(val));
                  setCurrentPage(1);
                }}
              >
                <SelectTrigger className="h-8 w-24 text-xs bg-background">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="10" className="text-xs">
                    10 dòng
                  </SelectItem>
                  <SelectItem value="25" className="text-xs">
                    25 dòng
                  </SelectItem>
                  <SelectItem value="50" className="text-xs">
                    50 dòng
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          {/* Table Container */}
          <div className="flex-1 overflow-auto">
            <table className="w-full border-collapse text-left text-xs">
              <thead className="sticky top-0 z-10 bg-muted/60 backdrop-blur-xs border-b border-border/60 text-[11px] font-semibold text-muted-foreground uppercase tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 w-8">
                    <Checkbox
                      checked={
                        paginatedTenants.length > 0 &&
                        selectedTenantIds.size === paginatedTenants.length
                      }
                      onCheckedChange={handleSelectAllTenants}
                      disabled={paginatedTenants.length === 0}
                      aria-label="Select all tenants"
                    />
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("name")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Tổ Chức / Không Gian</span>
                      {sortKey === "name" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("tier")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Gói Cước</span>
                      {sortKey === "tier" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("profilesUsed")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Dung Lượng Profiles</span>
                      {sortKey === "profilesUsed" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("membersUsed")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Thành Viên</span>
                      {sortKey === "membersUsed" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("status")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Trạng Thái</span>
                      {sortKey === "status" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th className="py-2.5 px-4 text-right">Thao Tác Quản Trị</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {paginatedTenants.length === 0 ? (
                  <tr>
                    <td
                      colSpan={7}
                      className="py-12 text-center text-muted-foreground"
                    >
                      Không tìm thấy tổ chức nào phù hợp với bộ lọc tìm kiếm.
                    </td>
                  </tr>
                ) : (
                  paginatedTenants.map((tenant) => {
                    const profilePct =
                      tenant.profilesUsed === null
                        ? null
                        : Math.round(
                            (tenant.profilesUsed /
                              Math.max(1, tenant.profilesMax)) *
                              100,
                          );
                    return (
                      <tr
                        key={tenant.id}
                        className={cn("transition-colors hover:bg-muted/30")}
                      >
                        <td className="py-3 px-3 w-8">
                          <Checkbox
                            checked={selectedTenantIds.has(tenant.id)}
                            onCheckedChange={() =>
                              handleToggleSelectTenant(tenant.id)
                            }
                            aria-label={`Select ${tenant.name}`}
                          />
                        </td>
                        <td className="py-3 px-4">
                          <div className="font-semibold text-foreground text-xs">
                            {tenant.name}
                          </div>
                          <div className="text-[11px] text-muted-foreground flex items-center gap-2 mt-0.5">
                            <span className="font-mono">{tenant.id}</span>
                            <span>•</span>
                            <span>{tenant.ownerEmail}</span>
                          </div>
                        </td>
                        <td className="py-3 px-4">
                          <Badge
                            variant="outline"
                            className={cn(
                              "text-[10px] font-bold font-mono tracking-wide",
                              tenant.tier === "ENTERPRISE"
                                ? "bg-amber-500/10 text-amber-600 border-amber-500/30"
                                : tenant.tier === "PRO TEAM"
                                  ? "bg-primary/10 text-primary border-primary/30"
                                  : "bg-muted text-muted-foreground border-border",
                            )}
                          >
                            {tenant.tier}
                          </Badge>
                        </td>
                        <td className="py-3 px-4">
                          <div className="flex items-center justify-between text-[11px] mb-1 font-mono">
                            <span className="font-semibold text-foreground">
                              {tenant.profilesUsed === null
                                ? "—"
                                : tenant.profilesUsed.toLocaleString()}{" "}
                              / {tenant.profilesMax.toLocaleString()}
                            </span>
                            <span className="text-[10px] text-muted-foreground">
                              {profilePct === null ? "—" : `${profilePct}%`}
                            </span>
                          </div>
                          <div className="h-1.5 w-36 rounded-full bg-muted overflow-hidden">
                            <div
                              className={cn(
                                "h-full rounded-full transition-all duration-300",
                                profilePct !== null && profilePct >= 90
                                  ? "bg-amber-500"
                                  : "bg-primary",
                              )}
                              style={{
                                width: `${Math.min(100, profilePct ?? 0)}%`,
                              }}
                            />
                          </div>
                        </td>
                        <td className="py-3 px-4">
                          <span className="font-mono text-xs font-semibold text-foreground">
                            {tenant.membersUsed === null
                              ? "—"
                              : tenant.membersUsed}{" "}
                            / {tenant.membersMax} ghế
                          </span>
                        </td>
                        <td className="py-3 px-4">
                          <Badge
                            className={cn(
                              "text-[10px] font-semibold",
                              "bg-muted text-muted-foreground border-border",
                            )}
                          >
                            Trạng thái chưa có
                          </Badge>
                        </td>
                        <td className="py-3 px-4 text-right whitespace-nowrap">
                          <div className="flex items-center justify-end gap-1.5">
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => handleOpenAdjustQuota(tenant)}
                              className="h-7 px-2.5 text-[11px] font-medium cursor-pointer"
                            >
                              Sửa Quota
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

          {/* Floating Bulk Action Bar for Tenants */}
          {selectedTenantIds.size > 0 && (
            <div className="flex flex-wrap items-center justify-between gap-2.5 border-t border-border/60 bg-card p-3 shadow-md animate-in fade-in slide-in-from-bottom-2 text-xs">
              <div className="flex items-center gap-2">
                <Badge variant="default" className="text-xs px-2.5 py-0.5">
                  {t("platformAdmin.bulkActions.selected", {
                    count: selectedTenantIds.size,
                    defaultValue: `Đã chọn ${selectedTenantIds.size} tổ chức`,
                  })}
                </Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedTenantIds(new Set())}
                  className="h-7 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  {t("platformAdmin.bulkActions.clear", {
                    defaultValue: "Bỏ chọn",
                  })}
                </Button>
              </div>

              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={handleBulkAddQuota}
                  disabled={isBulkExecuting}
                  className="h-7 text-xs gap-1 cursor-pointer"
                >
                  <LuPlus className="size-3 text-primary" />
                  <span>
                    {t("platformAdmin.bulkActions.addQuota", {
                      defaultValue: "Tăng hạn mức (+500)",
                    })}
                  </span>
                </Button>
              </div>
            </div>
          )}

          {/* Table Footer Pagination */}
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 bg-muted/15 px-4 py-2.5 text-xs text-muted-foreground">
            <div>
              Hiển thị{" "}
              <strong className="text-foreground">
                {filteredTenants.length === 0
                  ? 0
                  : (currentPage - 1) * pageSize + 1}
                -{Math.min(currentPage * pageSize, filteredTenants.length)}
              </strong>{" "}
              trong tổng số{" "}
              <strong className="text-foreground">
                {filteredTenants.length}
              </strong>{" "}
              tổ chức
            </div>

            <div className="flex items-center gap-1.5">
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage(1)}
                disabled={currentPage <= 1}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                « Đầu
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage((p) => Math.max(1, p - 1))}
                disabled={currentPage <= 1}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                ‹ Trước
              </Button>
              <span className="px-2 font-mono text-xs font-semibold text-foreground">
                Trang {currentPage} / {totalPages}
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  setCurrentPage((p) => Math.min(totalPages, p + 1))
                }
                disabled={currentPage >= totalPages}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                Sau ›
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage(totalPages)}
                disabled={currentPage >= totalPages}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                Cuối »
              </Button>
            </div>
          </div>
        </div>
      </div>

      {/* ACTION DIALOG: Adjust Quota */}
      <Dialog
        open={!!adjustTenant}
        onOpenChange={(open) => !open && setAdjustTenant(null)}
      >
        <DialogContent className="max-w-sm p-5">
          <DialogHeader className="border-b border-border/50 pb-2.5">
            <DialogTitle className="text-sm font-semibold">
              Điều Chỉnh Hạn Mức Profiles
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {adjustTenant?.name}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-3 py-2 text-xs">
            <div className="space-y-1">
              <Label className="text-xs font-medium">
                Hạn mức Profiles mới:
              </Label>
              <Input
                type="number"
                step={100}
                min={adjustTenant?.profilesUsed ?? 10}
                value={newMaxProfiles}
                onChange={(e) => setNewMaxProfiles(Number(e.target.value))}
                className="h-9 text-xs"
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              Hiện tại đang dùng:{" "}
              <strong className="text-foreground">
                {adjustTenant?.profilesUsed}
              </strong>{" "}
              profiles.
            </p>
          </div>

          <DialogFooter className="border-t border-border/50 pt-2.5">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setAdjustTenant(null)}
              className="h-8 text-xs cursor-pointer"
            >
              Hủy
            </Button>
            <Button
              type="button"
              variant="default"
              size="sm"
              onClick={handleSaveQuota}
              className="h-8 text-xs cursor-pointer font-medium"
            >
              Lưu thay đổi
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

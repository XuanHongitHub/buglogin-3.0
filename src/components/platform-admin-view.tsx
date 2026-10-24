"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
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
  organizationName: string;
  policy: Workspace["policy"];
  name: string;
  profilesMax: number;
  membersMax: number;
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
  const [loadError, setLoadError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [sortKey, setSortKey] = useState<keyof TenantRecord>("name");
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);

  // Quota adjust state
  const [adjustTenant, setAdjustTenant] = useState<TenantRecord | null>(null);
  const [newMaxProfiles, setNewMaxProfiles] = useState(0);
  const [isSavingQuota, setIsSavingQuota] = useState(false);

  // Multi-select for bulk tenant actions
  const [selectedTenantIds, setSelectedTenantIds] = useState<Set<string>>(
    new Set(),
  );
  const [isBulkExecuting, setIsBulkExecuting] = useState(false);

  // Load Real Data from Desktop / Backend
  const loadPlatformData = useCallback(async () => {
    setIsLoading(true);
    try {
      const organizations = await desktopApi.listOrganizations();
      const workspacePages = await Promise.all(
        organizations.items.map((organization) =>
          desktopApi.listWorkspaces(organization.id),
        ),
      );
      const organizationNames = new Map(
        organizations.items.map((organization) => [
          organization.id,
          organization.name,
        ]),
      );
      const mapped = workspacePages.flatMap((page) =>
        (page.items ?? []).map(
          (workspace): TenantRecord => ({
            id: workspace.id,
            organizationId: workspace.organizationId,
            organizationName:
              organizationNames.get(workspace.organizationId) ?? "—",
            policy: workspace.policy,
            name: workspace.name,
            profilesMax: workspace.policy.maxProfiles,
            membersMax: workspace.policy.maxMembers,
          }),
        ),
      );
      setTenants(mapped);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
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
      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        if (
          !tenant.name.toLowerCase().includes(q) &&
          !tenant.organizationName.toLowerCase().includes(q)
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
  }, [tenants, searchQuery, sortKey, sortDirection]);

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
      const failedIds: string[] = [];
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
          failedIds.push(tenant.id);
          console.warn(
            `Failed to update quota for workspace ${tenant.id}:`,
            err,
          );
        }
      }
      if (updated) await loadPlatformData();
      if (failedIds.length === 0)
        showSuccessToast(`Đã cập nhật quota cho ${updated} workspace.`);
      else
        showErrorToast(
          `Đã cập nhật ${updated}/${selected.length} workspace; ${failedIds.length} thao tác thất bại. Các workspace lỗi vẫn được chọn để thử lại.`,
        );
      setSelectedTenantIds(new Set(failedIds));
    } finally {
      setIsBulkExecuting(false);
    }
  };

  const handleOpenAdjustQuota = (tenant: TenantRecord) => {
    setAdjustTenant(tenant);
    setNewMaxProfiles(tenant.profilesMax);
  };

  const handleSaveQuota = async () => {
    if (
      !adjustTenant ||
      !Number.isInteger(newMaxProfiles) ||
      newMaxProfiles < 0
    )
      return;
    setIsSavingQuota(true);
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
    } finally {
      setIsSavingQuota(false);
    }
  };

  // These are configured policy limits, not live usage or billing entitlements.
  const totalProfilesMax = tenants.reduce((acc, t) => acc + t.profilesMax, 0);
  const totalMembersMax = tenants.reduce((acc, t) => acc + t.membersMax, 0);

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

      {/* 2. Workspace policy overview (configured limits, not live usage) */}
      <div className="border-b border-border/50 bg-muted/20 px-6 py-3 shrink-0">
        {loadError && (
          <div
            role="status"
            className="mb-3 flex items-center justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
          >
            <span>
              Không làm mới được dữ liệu. Đang giữ danh sách gần nhất.{" "}
              {loadError}
            </span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void loadPlatformData()}
              disabled={isLoading}
            >
              Thử lại
            </Button>
          </div>
        )}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <div className="rounded-md border border-border/70 bg-card px-3 py-2.5">
            <div className="text-xs text-muted-foreground">
              Workspace hiển thị
            </div>
            <div className="mt-1 font-mono text-sm font-semibold tabular-nums">
              {tenants.length.toLocaleString()}
            </div>
          </div>
          <div className="rounded-md border border-border/70 bg-card px-3 py-2.5">
            <div className="text-xs text-muted-foreground">
              Tổng hạn mức profile đã cấu hình
            </div>
            <div className="mt-1 flex items-center gap-2 font-mono text-sm font-semibold tabular-nums">
              <LuHardDrive className="size-4 text-muted-foreground" />
              {totalProfilesMax.toLocaleString()}
            </div>
          </div>
          <div className="rounded-md border border-border/70 bg-card px-3 py-2.5">
            <div className="text-xs text-muted-foreground">
              Tổng hạn mức thành viên đã cấu hình
            </div>
            <div className="mt-1 flex items-center gap-2 font-mono text-sm font-semibold tabular-nums">
              <LuUsers className="size-4 text-muted-foreground" />
              {totalMembersMax.toLocaleString()}
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
                  placeholder="Tìm theo workspace hoặc tổ chức…"
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
                    onClick={() => handleSort("profilesMax")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Hạn mức profile</span>
                      {sortKey === "profilesMax" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("membersMax")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>Hạn mức thành viên</span>
                      {sortKey === "membersMax" && (
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
                      colSpan={5}
                      className="py-12 text-center text-muted-foreground"
                    >
                      {isLoading
                        ? "Đang tải workspace…"
                        : "Không tìm thấy workspace nào phù hợp."}
                    </td>
                  </tr>
                ) : (
                  paginatedTenants.map((tenant) => (
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
                          <span>{tenant.organizationName}</span>
                        </div>
                      </td>
                      <td className="py-3 px-4">
                        <span className="font-mono text-xs tabular-nums">
                          {tenant.profilesMax.toLocaleString()}
                        </span>
                      </td>
                      <td className="py-3 px-4">
                        <span className="font-mono text-xs font-semibold text-foreground">
                          {tenant.membersMax.toLocaleString()}
                        </span>
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
                  ))
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
                min={0}
                value={newMaxProfiles}
                onChange={(e) => setNewMaxProfiles(Number(e.target.value))}
                className="h-9 text-xs"
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              Hạn mức profile hiện tại:{" "}
              <strong className="text-foreground">
                {adjustTenant?.profilesMax.toLocaleString()}
              </strong>{" "}
              profiles. API hiện tại không cung cấp số lượng đang dùng.
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
              disabled={
                isSavingQuota ||
                !Number.isInteger(newMaxProfiles) ||
                newMaxProfiles < 0
              }
              className="h-8 text-xs cursor-pointer font-medium"
            >
              {isSavingQuota ? "Đang lưu…" : "Lưu thay đổi"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

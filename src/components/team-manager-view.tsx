"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuActivity,
  LuArrowLeft,
  LuArrowRight,
  LuClock,
  LuLaptop,
  LuRefreshCw,
  LuSearch,
  LuShieldCheck,
  LuSquare,
  LuUsers,
  LuZap,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { awaitDesktopOperation } from "@/lib/await-desktop-operation";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type { BrowserProfile, GroupWithCount } from "@/types";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";

const desktopApi = createTauriDesktopApi();

export interface TeamOperatorSession {
  operatorId: string;
  name: string;
  email: string;
  role: string;
  profileId: string | null;
  profileName: string | null;
  proxyEndpoint: string | null;
  runtimeVersion: string;
  status: "running" | "idle" | "offline";
  groupScope: string;
}

interface TeamManagerViewProps {
  onNavigateHome: () => void;
  workspaceId?: string;
  workspaceName?: string;
  profiles?: BrowserProfile[];
  groups?: GroupWithCount[];
  onRefreshProfiles?: () => void;
}

export function TeamManagerView({
  onNavigateHome,
  workspaceId = "ws_default",
  workspaceName = "Workspace",
  profiles = [],
  onRefreshProfiles,
}: TeamManagerViewProps) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<TeamOperatorSession[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [stoppingProfileId, setStoppingProfileId] = useState<string | null>(
    null,
  );
  const [isBulkStopping, setIsBulkStopping] = useState(false);

  // Search & Filters
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [sortKey, setSortKey] = useState<keyof TeamOperatorSession>("name");
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);

  // Multi-selection for bulk actions
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // The API does not yet support audited profile handover.
  const [handoverModalOpen, setHandoverModalOpen] = useState(false);

  // Load Real Members and Active Profiles
  const loadTeamData = useCallback(async () => {
    setIsLoading(true);
    try {
      const page = await desktopApi.listMembers(workspaceId);
      const loadedMembers = page.items;
      const runningProfiles = profiles.filter((p) => p.state === "running");
      const memberRows: TeamOperatorSession[] = loadedMembers.map((member) => ({
        operatorId: member.id,
        name: member.identity?.username ?? "Danh tính chưa được cấp quyền xem",
        email: member.identity?.email ?? "—",
        role: member.role,
        profileId: null,
        profileName: null,
        proxyEndpoint: null,
        runtimeVersion: "—",
        status: member.status === "active" ? "idle" : "offline",
        groupScope: member.teamId ?? "Chưa gán team",
      }));
      const profileRows: TeamOperatorSession[] = runningProfiles.map(
        (profile) => {
          const proxyEndpoint = profile.proxy
            ? `${profile.proxy.protocol ?? "http"}://${profile.proxy.host}:${profile.proxy.port}`
            : "Direct";
          return {
            operatorId: `profile:${profile.id}`,
            name: profile.name,
            email: "—",
            role: "Profile session",
            profileId: profile.id,
            profileName: profile.name,
            proxyEndpoint,
            runtimeVersion: profile.runtimeVersion ?? "—",
            status: "running",
            groupScope: "—",
          };
        },
      );
      setSessions([...memberRows, ...profileRows]);
    } catch (err) {
      showErrorToast(
        t("teamManager.syncError", {
          defaultValue: `Lỗi đồng bộ dữ liệu: ${String(err)}`,
        }),
      );
    } finally {
      setIsLoading(false);
    }
  }, [workspaceId, profiles, t]);

  useEffect(() => {
    void loadTeamData();
  }, [loadTeamData]);

  // Single Stop Profile Command
  const handleStopProfile = async (session: TeamOperatorSession) => {
    if (!session.profileId) return;
    setStoppingProfileId(session.profileId);
    try {
      const accepted = await desktopApi.stopProfile(session.profileId);
      const operation = await awaitDesktopOperation(accepted, (operationId) =>
        desktopApi.getOperation(operationId),
      );
      if (operation.status !== "completed") {
        throw new Error(
          operation.error?.title ?? `Profile stop ${operation.status}`,
        );
      }
      showSuccessToast(
        t("teamManager.stopSuccess", {
          name: session.profileName,
          defaultValue: `Đã dừng profile "${session.profileName}"`,
        }),
      );
    } catch (err) {
      showErrorToast(
        t("teamManager.stopError", {
          defaultValue: `Không thể dừng profile: ${String(err)}`,
        }),
      );
    } finally {
      setStoppingProfileId(null);
      if (onRefreshProfiles) onRefreshProfiles();
      void loadTeamData();
    }
  };

  // Bulk Stop Selected Command
  const handleBulkStop = async () => {
    const activeToStop = sessions.filter(
      (s) => selectedIds.has(s.operatorId) && s.profileId,
    );
    if (activeToStop.length === 0) {
      showErrorToast(
        t("teamManager.noRunningSelected", {
          defaultValue:
            "Không có profile đang chạy nào trong số các mục đã chọn.",
        }),
      );
      return;
    }
    setIsBulkStopping(true);
    try {
      for (const item of activeToStop) {
        if (item.profileId) {
          const accepted = await desktopApi.stopProfile(item.profileId);
          const operation = await awaitDesktopOperation(
            accepted,
            (operationId) => desktopApi.getOperation(operationId),
          );
          if (operation.status !== "completed") {
            throw new Error(
              operation.error?.title ?? `Profile stop ${operation.status}`,
            );
          }
        }
      }
      showSuccessToast(
        t("teamManager.bulkStopSuccess", {
          count: activeToStop.length,
          defaultValue: `Đã dừng ${activeToStop.length} profile thành công.`,
        }),
      );
      setSelectedIds(new Set());
    } catch (err) {
      showErrorToast(
        t("teamManager.bulkStopError", {
          defaultValue: `Lỗi dừng hàng loạt: ${String(err)}`,
        }),
      );
    } finally {
      setIsBulkStopping(false);
      if (onRefreshProfiles) onRefreshProfiles();
      void loadTeamData();
    }
  };

  // Selection handlers
  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedIds(new Set(paginatedSessions.map((s) => s.operatorId)));
    } else {
      setSelectedIds(new Set());
    }
  };

  const handleToggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  // Sorting & Filtering
  const filteredSessions = useMemo(() => {
    let result = sessions.filter((s) => {
      if (statusFilter !== "all" && s.status !== statusFilter) {
        return false;
      }
      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        if (
          !s.name.toLowerCase().includes(q) &&
          !s.email.toLowerCase().includes(q) &&
          !(s.profileName?.toLowerCase().includes(q) ?? false) &&
          !(s.proxyEndpoint?.toLowerCase().includes(q) ?? false)
        ) {
          return false;
        }
      }
      return true;
    });

    result = [...result].sort((a, b) => {
      const aVal = a[sortKey] ?? "";
      const bVal = b[sortKey] ?? "";
      return sortDirection === "asc"
        ? String(aVal).localeCompare(String(bVal))
        : String(bVal).localeCompare(String(aVal));
    });

    return result;
  }, [sessions, statusFilter, searchQuery, sortKey, sortDirection]);

  // Pagination
  const totalPages = Math.max(1, Math.ceil(filteredSessions.length / pageSize));
  const paginatedSessions = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredSessions.slice(start, start + pageSize);
  }, [filteredSessions, currentPage, pageSize]);

  const handleSort = (key: keyof TeamOperatorSession) => {
    if (sortKey === key) {
      setSortDirection((prev) => (prev === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDirection("asc");
    }
    setCurrentPage(1);
  };

  const runningCount = sessions.filter((s) => s.status === "running").length;
  const idleCount = sessions.filter((s) => s.status === "idle").length;

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
              title={t("common.buttons.back", { defaultValue: "Quay lại" })}
            >
              <LuArrowLeft className="size-4" />
            </button>
            <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
              <LuUsers className="size-5" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h1 className="text-base font-semibold text-foreground">
                  {t("teamManager.title", {
                    defaultValue: "Quản Lý Đội Ngũ & Phân Bổ Hồ Sơ",
                  })}
                </h1>
                <Badge
                  variant="outline"
                  className="text-[10px] font-mono border-primary/30 text-primary"
                >
                  {t("teamManager.liveOperations", {
                    defaultValue: "Hoạt động trực tiếp",
                  })}
                </Badge>
              </div>
              <p className="text-xs text-muted-foreground mt-0.5">
                {t("header.workspaceLabel", { defaultValue: "Không gian" })}:{" "}
                <strong className="text-foreground">{workspaceName}</strong> •{" "}
                {t("teamManager.subtitle", {
                  defaultValue:
                    "Giám sát thành viên, phân bổ tài nguyên và kiểm soát phiên làm việc theo thời gian thực.",
                })}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                if (onRefreshProfiles) onRefreshProfiles();
                void loadTeamData();
              }}
              disabled={isLoading}
              className="h-8 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuRefreshCw
                className={cn("size-3.5", isLoading && "animate-spin")}
              />
              <span>
                {t("common.buttons.refresh", { defaultValue: "Làm mới" })}
              </span>
            </Button>
            <Button
              variant="default"
              size="sm"
              onClick={() => setHandoverModalOpen(true)}
              className="h-8 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuArrowRight className="size-3.5" />
              <span>
                {t("teamManager.quickHandover", {
                  defaultValue: "Bàn giao hồ sơ",
                })}
              </span>
            </Button>
          </div>
        </div>
      </div>

      {/* 2. Operations Telemetry */}
      <div className="border-b border-border/50 bg-muted/20 px-6 py-3 shrink-0">
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
          {/* Card 1: Running Sessions */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuActivity className="size-3.5 text-emerald-500" />
                <span>
                  {t("teamManager.activeProfiles", {
                    defaultValue: "Hồ sơ đang mở:",
                  })}
                </span>
              </span>
              <span className="font-bold text-emerald-500 font-mono text-sm">
                {runningCount}
              </span>
            </div>
            <div className="text-[10px] text-muted-foreground">
              {runningCount > 0
                ? t("teamManager.browserRunningActive", {
                    defaultValue: "Trình duyệt đang vận hành tích cực",
                  })
                : t("teamManager.noProfilesRunning", {
                    defaultValue: "Chưa có profile nào được mở",
                  })}
            </div>
          </div>

          {/* Card 2: Idle Members */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuClock className="size-3.5 text-amber-500" />
                <span>
                  {t("teamManager.idleMembers", {
                    defaultValue: "Thành viên sẵn sàng:",
                  })}
                </span>
              </span>
              <span className="font-bold text-foreground font-mono text-sm">
                {idleCount}
              </span>
            </div>
            <div className="text-[10px] text-muted-foreground">
              {t("teamManager.readyForHandover", {
                defaultValue: "Sẵn sàng nhận phân công hồ sơ",
              })}
            </div>
          </div>

          {/* Card 3: DLP Enforcement */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuShieldCheck className="size-3.5 text-primary" />
                <span>
                  {t("teamManager.dlpPolicy", { defaultValue: "Bảo mật DLP:" })}
                </span>
              </span>
              <Badge className="text-[9px] font-semibold bg-emerald-500/10 text-emerald-600 border-emerald-500/30">
                ACTIVE
              </Badge>
            </div>
            <div className="text-[10px] text-muted-foreground">
              {t("teamManager.dlpDesc", {
                defaultValue: "Khóa Raw Cookie & Ẩn mật khẩu Proxy",
              })}
            </div>
          </div>

          {/* Card 4: Fingerprint Isolation */}
          <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1 shadow-2xs">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground font-medium flex items-center gap-1.5">
                <LuZap className="size-3.5 text-primary" />
                <span>
                  {t("teamManager.fingerprintIsolation", {
                    defaultValue: "Cô lập vân tay:",
                  })}
                </span>
              </span>
              <span className="font-bold text-foreground font-mono text-xs">
                BugLogin Core
              </span>
            </div>
            <div className="text-[10px] text-muted-foreground">
              Canvas, WebGL, WebRTC 0-Leak
            </div>
          </div>
        </div>
      </div>

      {/* Floating Bulk Action Toolbar */}
      {selectedIds.size > 0 && (
        <div className="mx-6 mt-3 flex items-center justify-between gap-3 px-4 py-2 bg-primary/10 border border-primary/30 rounded-lg shadow-sm">
          <div className="flex items-center gap-2">
            <Badge variant="default" className="font-mono text-xs">
              {t("teamManager.bulkSelected", {
                count: selectedIds.size,
                defaultValue: `Đã chọn ${selectedIds.size} mục`,
              })}
            </Badge>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="destructive"
              size="sm"
              onClick={() => void handleBulkStop()}
              disabled={isBulkStopping}
              className="h-7 px-3 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuSquare className="size-3" />
              <span>
                {isBulkStopping
                  ? t("teamManager.stopping", { defaultValue: "Đang dừng..." })
                  : t("teamManager.bulkStop", {
                      defaultValue: "Dừng khẩn cấp các phiên đã chọn",
                    })}
              </span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setHandoverModalOpen(true)}
              className="h-7 px-3 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuArrowRight className="size-3" />
              <span>
                {t("teamManager.bulkHandover", {
                  defaultValue: "Bàn giao hồ sơ",
                })}
              </span>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setSelectedIds(new Set())}
              className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
            >
              {t("common.buttons.clear", { defaultValue: "Bỏ chọn" })}
            </Button>
          </div>
        </div>
      )}

      {/* 3. Main Data Area */}
      <div className="flex-1 min-h-0 p-6 flex flex-col">
        <div className="rounded-xl border border-border/70 bg-card shadow-sm flex flex-col flex-1 min-h-0 overflow-hidden">
          {/* Filter Bar */}
          <div className="p-3 border-b border-border/50 bg-muted/10 flex flex-wrap items-center justify-between gap-3 shrink-0">
            <div className="flex items-center gap-2 flex-1 max-w-sm">
              <div className="relative w-full">
                <LuSearch className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
                <Input
                  placeholder={t("common.searchPlaceholder", {
                    defaultValue: "Tìm kiếm thành viên, email hoặc profile...",
                  })}
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
                value={statusFilter}
                onValueChange={(val) => {
                  setStatusFilter(val);
                  setCurrentPage(1);
                }}
              >
                <SelectTrigger className="h-8 w-36 text-xs bg-background">
                  <SelectValue
                    placeholder={t("teamManager.table.status", {
                      defaultValue: "Trạng thái",
                    })}
                  />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all" className="text-xs">
                    {t("common.filter.all", {
                      defaultValue: "Tất cả trạng thái",
                    })}
                  </SelectItem>
                  <SelectItem value="running" className="text-xs">
                    {t("roles.statusRunning", {
                      defaultValue: "Đang hoạt động",
                    })}
                  </SelectItem>
                  <SelectItem value="idle" className="text-xs">
                    {t("roles.statusReady", { defaultValue: "Sẵn sàng" })}
                  </SelectItem>
                  <SelectItem value="offline" className="text-xs">
                    {t("roles.statusOffline", { defaultValue: "Ngoại tuyến" })}
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
                    10
                  </SelectItem>
                  <SelectItem value="25" className="text-xs">
                    25
                  </SelectItem>
                  <SelectItem value="50" className="text-xs">
                    50
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          {/* Table Area */}
          <div className="flex-1 overflow-auto">
            <table className="w-full border-collapse text-left text-xs">
              <thead className="sticky top-0 z-10 bg-muted/60 backdrop-blur-xs border-b border-border/60 text-[11px] font-semibold text-muted-foreground uppercase tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 w-10">
                    <Checkbox
                      checked={
                        paginatedSessions.length > 0 &&
                        paginatedSessions.every((s) =>
                          selectedIds.has(s.operatorId),
                        )
                      }
                      onCheckedChange={handleSelectAll}
                      aria-label="Select all"
                    />
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("name")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>
                        {t("teamManager.table.member", {
                          defaultValue: "Thành viên",
                        })}
                      </span>
                      {sortKey === "name" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("profileName")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>
                        {t("teamManager.table.activeProfile", {
                          defaultValue: "Hồ sơ đang mở",
                        })}
                      </span>
                      {sortKey === "profileName" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th className="py-2.5 px-4">
                    {t("teamManager.table.proxy", {
                      defaultValue: "Proxy định tuyến",
                    })}
                  </th>
                  <th className="py-2.5 px-4">
                    {t("teamManager.table.scope", {
                      defaultValue: "Phạm vi nhóm",
                    })}
                  </th>
                  <th
                    className="py-2.5 px-4 cursor-pointer select-none hover:text-foreground transition-colors"
                    onClick={() => handleSort("status")}
                  >
                    <div className="flex items-center gap-1.5">
                      <span>
                        {t("teamManager.table.status", {
                          defaultValue: "Trạng thái hoạt động",
                        })}
                      </span>
                      {sortKey === "status" && (
                        <span>{sortDirection === "asc" ? "▲" : "▼"}</span>
                      )}
                    </div>
                  </th>
                  <th className="py-2.5 px-4 text-right">
                    {t("teamManager.table.actions", {
                      defaultValue: "Hành động",
                    })}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {paginatedSessions.length === 0 ? (
                  <tr>
                    <td
                      colSpan={7}
                      className="py-12 text-center text-muted-foreground"
                    >
                      {t("teamManager.noMatchingSessions", {
                        defaultValue:
                          "Không có thành viên hoặc phiên làm việc nào phù hợp với bộ lọc.",
                      })}
                    </td>
                  </tr>
                ) : (
                  paginatedSessions.map((session) => {
                    const isRunning = session.status === "running";
                    const isSelected = selectedIds.has(session.operatorId);

                    return (
                      <tr
                        key={session.operatorId}
                        className={cn(
                          "transition-colors hover:bg-muted/30",
                          isSelected && "bg-primary/5",
                        )}
                      >
                        <td className="py-3 px-3">
                          <Checkbox
                            checked={isSelected}
                            onCheckedChange={() =>
                              handleToggleSelect(session.operatorId)
                            }
                            aria-label={`Select ${session.name}`}
                          />
                        </td>
                        <td className="py-3 px-4">
                          <div className="font-semibold text-foreground text-xs flex items-center gap-1.5">
                            <span>{session.name}</span>
                            <Badge
                              variant="outline"
                              className="text-[9px] uppercase font-mono py-0 h-4"
                            >
                              {session.role}
                            </Badge>
                          </div>
                          <div className="text-[11px] text-muted-foreground mt-0.5">
                            {session.email}
                          </div>
                          <div className="text-[10px] text-muted-foreground/50 font-mono">
                            #{session.operatorId}
                          </div>
                        </td>
                        <td className="py-3 px-4">
                          {isRunning && session.profileName ? (
                            <div>
                              <div className="font-semibold text-foreground text-xs flex items-center gap-1.5">
                                <LuLaptop className="size-3.5 text-primary" />
                                <span>{session.profileName}</span>
                              </div>
                              <code className="text-[10px] text-muted-foreground font-mono">
                                {session.profileId}
                              </code>
                            </div>
                          ) : (
                            <span className="text-muted-foreground italic text-[11px]">
                              {t("teamManager.noProfileOpen", {
                                defaultValue: "Chưa mở profile",
                              })}
                            </span>
                          )}
                        </td>
                        <td className="py-3 px-4">
                          {isRunning && session.proxyEndpoint ? (
                            <span className="font-mono text-[11px] bg-muted/60 px-2 py-0.5 rounded border border-border/50">
                              {session.proxyEndpoint}
                            </span>
                          ) : (
                            <span className="text-muted-foreground text-[11px]">
                              --
                            </span>
                          )}
                        </td>
                        <td className="py-3 px-4">
                          <Badge
                            variant="outline"
                            className="text-[10px] text-muted-foreground"
                          >
                            {session.groupScope}
                          </Badge>
                        </td>
                        <td className="py-3 px-4">
                          <Badge
                            className={cn(
                              "text-[10px] font-semibold gap-1",
                              isRunning
                                ? "bg-emerald-500/10 text-emerald-600 border-emerald-500/30"
                                : session.status === "idle"
                                  ? "bg-amber-500/10 text-amber-600 border-amber-500/30"
                                  : "bg-muted text-muted-foreground border-border",
                            )}
                          >
                            {isRunning && (
                              <span className="size-1.5 rounded-full bg-emerald-500 animate-pulse" />
                            )}
                            <span>
                              {isRunning
                                ? t("roles.statusRunning", {
                                    defaultValue: "Đang hoạt động",
                                  })
                                : session.status === "idle"
                                  ? t("roles.statusReady", {
                                      defaultValue: "Sẵn sàng",
                                    })
                                  : t("roles.statusOffline", {
                                      defaultValue: "Ngoại tuyến",
                                    })}
                            </span>
                          </Badge>
                        </td>
                        <td className="py-3 px-4 text-right whitespace-nowrap">
                          {isRunning && session.profileId ? (
                            <Button
                              variant="destructive"
                              size="sm"
                              onClick={() => void handleStopProfile(session)}
                              disabled={
                                isBulkStopping ||
                                stoppingProfileId === session.profileId
                              }
                              className="h-7 px-2.5 text-[11px] gap-1 cursor-pointer font-medium"
                            >
                              <LuSquare className="size-3" />
                              <span>
                                {isBulkStopping ||
                                stoppingProfileId === session.profileId
                                  ? t("teamManager.stopping", {
                                      defaultValue: "Đang dừng...",
                                    })
                                  : t("teamManager.stopAction", {
                                      defaultValue: "Dừng khẩn cấp",
                                    })}
                              </span>
                            </Button>
                          ) : (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => setHandoverModalOpen(true)}
                              className="h-7 px-2.5 text-[11px] cursor-pointer font-medium"
                            >
                              {t("teamManager.assignProfile", {
                                defaultValue: "Gán hồ sơ",
                              })}
                            </Button>
                          )}
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>

          {/* Table Footer */}
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 bg-muted/15 px-4 py-2.5 text-xs text-muted-foreground">
            <div>
              {t("common.pagination.showing", { defaultValue: "Hiển thị" })}{" "}
              <strong className="text-foreground">
                {filteredSessions.length === 0
                  ? 0
                  : (currentPage - 1) * pageSize + 1}
                -{Math.min(currentPage * pageSize, filteredSessions.length)}
              </strong>{" "}
              /{" "}
              <strong className="text-foreground">
                {filteredSessions.length}
              </strong>
            </div>

            <div className="flex items-center gap-1.5">
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage(1)}
                disabled={currentPage <= 1}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                «
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage((p) => Math.max(1, p - 1))}
                disabled={currentPage <= 1}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                ‹
              </Button>
              <span className="px-2 font-mono text-xs font-semibold text-foreground">
                {currentPage} / {totalPages}
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
                ›
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setCurrentPage(totalPages)}
                disabled={currentPage >= totalPages}
                className="h-7 px-2 text-xs cursor-pointer"
              >
                »
              </Button>
            </div>
          </div>
        </div>
      </div>

      {/* Explain the unsupported operation without presenting an editable draft. */}
      <Dialog open={handoverModalOpen} onOpenChange={setHandoverModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="flex items-center gap-2 text-base font-semibold">
              <LuArrowRight className="size-4 text-primary" />
              {t("teamManager.handoverTitle", {
                defaultValue: "Bàn giao profile",
              })}
            </DialogTitle>
            <DialogDescription className="text-sm text-muted-foreground">
              Chưa thể bàn giao profile vì API chưa hỗ trợ chuyển quyền có kiểm
              tra phạm vi và ghi audit. Không có thay đổi nào được thực hiện.
            </DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>
    </div>
  );
}

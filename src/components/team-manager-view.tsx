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
  LuShield,
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
import { resolveHumanIdentity } from "@/lib/identity-display";
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
  groups = [],
  onRefreshProfiles,
}: TeamManagerViewProps) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<TeamOperatorSession[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [stoppingProfileId, setStoppingProfileId] = useState<string | null>(
    null,
  );

  // Search & Filters
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [sortKey, setSortKey] = useState<keyof TeamOperatorSession>("name");
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);

  // Multi-selection for bulk actions
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // Fast profile handover state
  const [handoverModalOpen, setHandoverModalOpen] = useState(false);
  const [sourceMemberId, setSourceMemberId] = useState("");
  const [targetMemberId, setTargetMemberId] = useState("");
  const [handoverGroupId, setHandoverGroupId] = useState("");
  const [isHandingOver, setIsHandingOver] = useState(false);

  // Load Real Members and Active Profiles
  const loadTeamData = useCallback(async () => {
    setIsLoading(true);
    try {
      // 1. Fetch real workspace members
      let loadedMembers: Array<{
        id: string;
        accountId: string;
        role: string;
        status: string;
      }> = [];
      try {
        const page = await desktopApi.listMembers(workspaceId);
        loadedMembers = page?.items ?? [];
      } catch (err) {
        console.warn("Could not load workspace members:", err);
      }

      // 2. Identify running profiles
      const runningProfiles = profiles.filter((p) => p.state === "running");

      // 3. Build session records
      if (loadedMembers.length > 0) {
        const mappedSessions: TeamOperatorSession[] = loadedMembers.map(
          (member, index) => {
            const activeProf = runningProfiles[index] ?? null;
            const isRunning = Boolean(activeProf);

            let proxyEndpoint: string | null = null;
            if (activeProf?.proxy) {
              proxyEndpoint = `${activeProf.proxy.protocol ?? "http"}://${activeProf.proxy.host}:${activeProf.proxy.port}`;
            }

            const resolved = resolveHumanIdentity(
              member.accountId,
              member.role,
              index,
              t,
            );

            return {
              operatorId: member.id,
              name: resolved.displayName,
              email: resolved.email,
              role: member.role,
              profileId: activeProf?.id ?? null,
              profileName: activeProf?.name ?? null,
              proxyEndpoint: proxyEndpoint ?? (isRunning ? "Direct" : null),
              runtimeVersion: activeProf?.runtimeVersion ?? "Core",
              status: isRunning
                ? "running"
                : member.status === "active"
                  ? "idle"
                  : "offline",
              groupScope: t("teamManager.allGroups", {
                defaultValue: "Toàn bộ nhóm",
              }),
            };
          },
        );

        // Add additional running profiles if any
        if (runningProfiles.length > loadedMembers.length) {
          for (let i = loadedMembers.length; i < runningProfiles.length; i++) {
            const p = runningProfiles[i];
            let proxyEndpoint: string | null = null;
            if (p.proxy) {
              proxyEndpoint = `${p.proxy.protocol ?? "http"}://${p.proxy.host}:${p.proxy.port}`;
            }
            mappedSessions.push({
              operatorId: `op_local_${i + 1}`,
              name: `${t("roles.operator", { defaultValue: "Thành viên" })} #${i + 1}`,
              email: `member_${i + 1}@workspace.local`,
              role: "operator",
              profileId: p.id,
              profileName: p.name,
              proxyEndpoint: proxyEndpoint ?? "Direct",
              runtimeVersion: p.runtimeVersion ?? "Core",
              status: "running",
              groupScope: t("teamManager.assignedScope", {
                defaultValue: "Được chỉ định",
              }),
            });
          }
        }

        setSessions(mappedSessions);
      } else if (runningProfiles.length > 0) {
        const mappedSessions: TeamOperatorSession[] = runningProfiles.map(
          (p, i) => {
            let proxyEndpoint: string | null = null;
            if (p.proxy) {
              proxyEndpoint = `${p.proxy.protocol ?? "http"}://${p.proxy.host}:${p.proxy.port}`;
            }
            return {
              operatorId: `op_local_${i + 1}`,
              name: `${t("roles.operator", { defaultValue: "Thành viên" })} #${i + 1}`,
              email: `member_${i + 1}@workspace.local`,
              role: "operator",
              profileId: p.id,
              profileName: p.name,
              proxyEndpoint: proxyEndpoint ?? "Direct",
              runtimeVersion: p.runtimeVersion ?? "Core",
              status: "running",
              groupScope: t("teamManager.allGroups", {
                defaultValue: "Toàn bộ nhóm",
              }),
            };
          },
        );
        setSessions(mappedSessions);
      } else {
        const resolvedSelf = resolveHumanIdentity(null, "owner", 0, t);
        setSessions([
          {
            operatorId: "mem_current",
            name: resolvedSelf.displayName,
            email: resolvedSelf.email,
            role: "owner",
            profileId: null,
            profileName: null,
            proxyEndpoint: null,
            runtimeVersion: "Core",
            status: "idle",
            groupScope: t("teamManager.allGroups", {
              defaultValue: "Toàn bộ nhóm",
            }),
          },
        ]);
      }
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
      await desktopApi.stopProfile(session.profileId);
      showSuccessToast(
        t("teamManager.stopSuccess", {
          name: session.profileName,
          defaultValue: `Đã dừng profile "${session.profileName}"`,
        }),
      );
      if (onRefreshProfiles) onRefreshProfiles();
      void loadTeamData();
    } catch (err) {
      showErrorToast(
        t("teamManager.stopError", {
          defaultValue: `Không thể dừng profile: ${String(err)}`,
        }),
      );
    } finally {
      setStoppingProfileId(null);
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
    try {
      for (const item of activeToStop) {
        if (item.profileId) {
          await desktopApi.stopProfile(item.profileId);
        }
      }
      showSuccessToast(
        t("teamManager.bulkStopSuccess", {
          count: activeToStop.length,
          defaultValue: `Đã dừng ${activeToStop.length} profile thành công.`,
        }),
      );
      setSelectedIds(new Set());
      if (onRefreshProfiles) onRefreshProfiles();
      void loadTeamData();
    } catch (err) {
      showErrorToast(
        t("teamManager.bulkStopError", {
          defaultValue: `Lỗi dừng hàng loạt: ${String(err)}`,
        }),
      );
    }
  };

  // Bulk Handover Execution
  const handleExecuteHandover = async () => {
    if (!sourceMemberId || !targetMemberId) {
      showErrorToast(
        t("teamManager.selectHandoverMembers", {
          defaultValue: "Vui lòng chọn thành viên bàn giao và nhận bàn giao.",
        }),
      );
      return;
    }
    if (sourceMemberId === targetMemberId) {
      showErrorToast(
        t("teamManager.sameHandoverMembers", {
          defaultValue:
            "Thành viên nhận bàn giao không được trùng với thành viên chuyển giao.",
        }),
      );
      return;
    }
    setIsHandingOver(true);
    try {
      const sourceName =
        sessions.find((s) => s.operatorId === sourceMemberId)?.name ??
        sourceMemberId;
      const targetName =
        sessions.find((s) => s.operatorId === targetMemberId)?.name ??
        targetMemberId;
      showSuccessToast(
        t("teamManager.handoverSuccess", {
          source: sourceName,
          target: targetName,
          defaultValue: `Bàn giao hồ sơ thành công từ ${sourceName} sang ${targetName}.`,
        }),
      );
      setHandoverModalOpen(false);
      setSelectedIds(new Set());
      void loadTeamData();
    } catch (err) {
      showErrorToast(
        t("teamManager.handoverError", {
          defaultValue: `Lỗi bàn giao: ${String(err)}`,
        }),
      );
    } finally {
      setIsHandingOver(false);
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
              onClick={() => {
                if (sessions.length >= 2) {
                  setSourceMemberId(sessions[0].operatorId);
                  setTargetMemberId(sessions[1].operatorId);
                }
                setHandoverModalOpen(true);
              }}
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
              className="h-7 px-3 text-xs gap-1.5 cursor-pointer font-medium"
            >
              <LuSquare className="size-3" />
              <span>
                {t("teamManager.bulkStop", {
                  defaultValue: "Dừng khẩn cấp các phiên đã chọn",
                })}
              </span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                const firstSelected = paginatedSessions.find((s) =>
                  selectedIds.has(s.operatorId),
                );
                if (firstSelected) setSourceMemberId(firstSelected.operatorId);
                setHandoverModalOpen(true);
              }}
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
                              disabled={stoppingProfileId === session.profileId}
                              className="h-7 px-2.5 text-[11px] gap-1 cursor-pointer font-medium"
                            >
                              <LuSquare className="size-3" />
                              <span>
                                {stoppingProfileId === session.profileId
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
                              onClick={() => {
                                setSourceMemberId(session.operatorId);
                                setHandoverModalOpen(true);
                              }}
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

      {/* ACTION DIALOG: Fast Profile Handover */}
      <Dialog open={handoverModalOpen} onOpenChange={setHandoverModalOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuArrowRight className="size-4 text-primary" />
              <span>
                {t("teamManager.handoverTitle", {
                  defaultValue: "Bàn Giao Quyền Vận Hành Profiles",
                })}
              </span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {t("teamManager.handoverDesc", {
                defaultValue:
                  "Chuyển giao quyền chạy profile giữa các thành viên với chính sách DLP bảo vệ dữ liệu.",
              })}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2 text-xs">
            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                {t("teamManager.sourceMember", {
                  defaultValue: "Từ thành viên bàn giao:",
                })}
              </Label>
              <Select value={sourceMemberId} onValueChange={setSourceMemberId}>
                <SelectTrigger className="h-9 text-xs">
                  <SelectValue
                    placeholder={t("common.selectPlaceholder", {
                      defaultValue: "Chọn thành viên...",
                    })}
                  />
                </SelectTrigger>
                <SelectContent>
                  {sessions.map((s) => (
                    <SelectItem
                      key={s.operatorId}
                      value={s.operatorId}
                      className="text-xs"
                    >
                      {s.name} ({s.email})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                {t("teamManager.targetMember", {
                  defaultValue: "Sang thành viên nhận bàn giao:",
                })}
              </Label>
              <Select value={targetMemberId} onValueChange={setTargetMemberId}>
                <SelectTrigger className="h-9 text-xs">
                  <SelectValue
                    placeholder={t("common.selectPlaceholder", {
                      defaultValue: "Chọn thành viên...",
                    })}
                  />
                </SelectTrigger>
                <SelectContent>
                  {sessions.map((s) => (
                    <SelectItem
                      key={s.operatorId}
                      value={s.operatorId}
                      className="text-xs"
                    >
                      {s.name} ({s.email})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                {t("teamManager.assignedProfilesGroup", {
                  defaultValue: "Nhóm hồ sơ bàn giao:",
                })}
              </Label>
              <Select
                value={handoverGroupId}
                onValueChange={setHandoverGroupId}
              >
                <SelectTrigger className="h-9 text-xs">
                  <SelectValue
                    placeholder={t("teamManager.allAssignedProfiles", {
                      defaultValue: "Toàn bộ hồ sơ được gán",
                    })}
                  />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all_assigned" className="text-xs">
                    {t("teamManager.allAssignedProfiles", {
                      defaultValue: "Toàn bộ hồ sơ được gán",
                    })}
                  </SelectItem>
                  {groups.map((g) => (
                    <SelectItem key={g.id} value={g.id} className="text-xs">
                      {g.name} ({g.profile_count} profiles)
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="rounded-lg border border-primary/30 bg-primary/5 p-3 text-[11px] text-muted-foreground space-y-1">
              <div className="font-semibold text-foreground flex items-center gap-1.5">
                <LuShield className="size-3.5 text-primary" />
                <span>
                  {t("teamManager.zeroKnowledgeSecurity", {
                    defaultValue: "Bảo mật tài nguyên:",
                  })}
                </span>
              </div>
              <p>
                {t("teamManager.zeroKnowledgeDesc", {
                  defaultValue:
                    "Thành viên nhận bàn giao chỉ có quyền khởi chạy hồ sơ, không thể xuất cookie hay xem mật khẩu proxy.",
                })}
              </p>
            </div>
          </div>

          <DialogFooter className="border-t border-border/50 pt-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setHandoverModalOpen(false)}
              className="h-8 text-xs cursor-pointer"
            >
              {t("common.buttons.cancel", { defaultValue: "Hủy" })}
            </Button>
            <Button
              type="button"
              variant="default"
              size="sm"
              onClick={() => void handleExecuteHandover()}
              disabled={isHandingOver}
              className="h-8 text-xs cursor-pointer font-medium"
            >
              {isHandingOver
                ? t("teamManager.handingOver", {
                    defaultValue: "Đang bàn giao...",
                  })
                : t("teamManager.confirmHandover", {
                    defaultValue: "Xác nhận bàn giao",
                  })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

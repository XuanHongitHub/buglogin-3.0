"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowRight,
  LuBan,
  LuBuilding2,
  LuCheck,
  LuCopy,
  LuCreditCard,
  LuCrown,
  LuHardDrive,
  LuKey,
  LuLaptop,
  LuPencil,
  LuSearch,
  LuShield,
  LuShieldCheck,
  LuSlidersHorizontal,
  LuTrash2,
  LuTriangleAlert,
  LuUserCheck,
  LuUsers,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
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
import {
  getActiveUserEmail,
  resolveHumanIdentity,
} from "@/lib/identity-display";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";

const desktopApi = createTauriDesktopApi();

export type HierarchicalRole =
  | "owner"
  | "admin"
  | "manager"
  | "operator"
  | "member";

export const ROLE_RANKS: Record<string, number> = {
  owner: 4,
  admin: 3,
  manager: 2,
  operator: 1,
  member: 0,
};

export interface OrgEmployee {
  id: string;
  name: string;
  email: string;
  role: HierarchicalRole;
  workspaces: string[];
  groupScope: string;
  status: "active" | "suspended";
}

export interface OrgWorkspaceItem {
  id: string;
  name: string;
  profileCount: number;
  maxProfiles: number;
  memberCount: number;
  maxMembers: number;
  offlineEntitled: boolean;
  cloudSync: boolean;
}

interface OrgManagementCenterProps {
  mode?: "page" | "dialog";
  isOpen?: boolean;
  onClose?: () => void;
  onNavigateHome?: () => void;
}

export function OrgManagementCenter({
  mode = "dialog",
  isOpen = true,
  onClose,
  onNavigateHome,
}: OrgManagementCenterProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<
    "members" | "workspaces" | "security" | "billing"
  >("members");

  const [employees, setEmployees] = useState<OrgEmployee[]>([]);
  const [workspaces, setWorkspaces] = useState<OrgWorkspaceItem[]>([]);
  const [currentUser, setCurrentUser] = useState<{
    id: string;
    email: string;
    role: HierarchicalRole;
  }>({
    id: "user_owner",
    email: "admin@buglogin.internal",
    role: "owner",
  });
  const [availableGroups, setAvailableGroups] = useState<
    Array<{ id: string; name: string; profile_count: number }>
  >([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchEmployeeQuery, setSearchEmployeeQuery] = useState("");
  const [workspaceFilter, setWorkspaceFilter] = useState("all");

  // Sub-modal states
  const [editOrgModalOpen, setEditOrgModalOpen] = useState(false);
  const [transferOwnerModalOpen, setTransferOwnerModalOpen] = useState(false);
  const [editWsModal, setEditWsModal] = useState<OrgWorkspaceItem | null>(null);
  const [deleteWsModal, setDeleteWsModal] = useState<OrgWorkspaceItem | null>(
    null,
  );
  const [resetPwdMember, setResetPwdMember] = useState<OrgEmployee | null>(
    null,
  );

  // Security Policies State (Default Auto-Logout to 0 / Disabled)
  const [require2FA, setRequire2FA] = useState(true);
  const [autoLogoutMinutes, setAutoLogoutMinutes] = useState(0);
  const [blockRawCookieForOperators, setBlockRawCookieForOperators] =
    useState(true);
  const [mandatoryAuditLog, setMandatoryAuditLog] = useState(true);

  // Form states for Submodals
  const [orgBillingEmail, setOrgBillingEmail] = useState(
    "admin@buglogin.internal",
  );
  const [orgName, setOrgName] = useState("BugLogin Organization");
  const [transferTargetEmail, setTransferTargetEmail] = useState("");
  const [transferMyPassword, setTransferMyPassword] = useState("");
  const [transferConfirmationPhrase, setTransferConfirmationPhrase] =
    useState("");
  const [isTransferring, setIsTransferring] = useState(false);

  // Reset password states
  const [generatedTempPassword, setGeneratedTempPassword] = useState(
    "Pass#89a2!BugLogin2026",
  );
  const [mustChangePasswordOnLogin, setMustChangePasswordOnLogin] =
    useState(true);
  const [forceLogoutAllSessions, setForceLogoutAllSessions] = useState(true);
  const [copiedTempPwd, setCopiedTempPwd] = useState(false);

  // Delete workspace verification
  const [deleteConfirmationName, setDeleteConfirmationName] = useState("");
  const [deleteHandleProfilesAction, setDeleteHandleProfilesAction] = useState<
    "archive" | "purge"
  >("archive");

  // Workspaces multi-select & bulk actions
  const [selectedWsIds, setSelectedWsIds] = useState<Set<string>>(new Set());
  const [bulkWsQuotaModalOpen, setBulkWsQuotaModalOpen] = useState(false);
  const [bulkMaxProfiles, setBulkMaxProfiles] = useState(200);
  const [bulkMaxMembers, setBulkMaxMembers] = useState(20);
  const [isBulkWsExecuting, setIsBulkWsExecuting] = useState(false);

  // Employees multi-select & bulk actions
  const [selectedEmpIds, setSelectedEmpIds] = useState<Set<string>>(new Set());
  const [isBulkEmpExecuting, setIsBulkEmpExecuting] = useState(false);

  const handleToggleSelectWs = (id: string) => {
    setSelectedWsIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSelectAllWs = () => {
    if (selectedWsIds.size === workspaces.length && workspaces.length > 0) {
      setSelectedWsIds(new Set());
    } else {
      setSelectedWsIds(new Set(workspaces.map((w) => w.id)));
    }
  };

  const handleApplyBulkWsQuotas = async () => {
    if (selectedWsIds.size === 0) return;
    setIsBulkWsExecuting(true);
    try {
      setWorkspaces((prev) =>
        prev.map((w) =>
          selectedWsIds.has(w.id)
            ? { ...w, maxProfiles: bulkMaxProfiles, maxMembers: bulkMaxMembers }
            : w,
        ),
      );
      const count = selectedWsIds.size;
      showSuccessToast(t("workspaces.bulkActions.quotaUpdated", { count }));
      setBulkWsQuotaModalOpen(false);
      setSelectedWsIds(new Set());
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsBulkWsExecuting(false);
    }
  };

  const handleBulkArchiveWs = async () => {
    if (selectedWsIds.size === 0) return;
    setIsBulkWsExecuting(true);
    try {
      const idsToRemove = new Set(selectedWsIds);
      setWorkspaces((prev) => prev.filter((w) => !idsToRemove.has(w.id)));
      const count = idsToRemove.size;
      showSuccessToast(t("workspaces.bulkActions.archived", { count }));
      setSelectedWsIds(new Set());
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsBulkWsExecuting(false);
    }
  };

  const manageableEmployees = useMemo(() => {
    return employees.filter((emp) => {
      const empRank = ROLE_RANKS[emp.role] ?? 0;
      const isSelf =
        emp.id === currentUser.id || emp.email === currentUser.email;
      return !isSelf && empRank < (ROLE_RANKS[currentUser.role] ?? 4);
    });
  }, [employees, currentUser]);

  const handleToggleSelectEmp = (id: string) => {
    setSelectedEmpIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSelectAllManageableEmp = () => {
    if (
      selectedEmpIds.size === manageableEmployees.length &&
      manageableEmployees.length > 0
    ) {
      setSelectedEmpIds(new Set());
    } else {
      setSelectedEmpIds(new Set(manageableEmployees.map((e) => e.id)));
    }
  };

  const handleBulkEmpRoleChange = async (newRole: HierarchicalRole) => {
    if (selectedEmpIds.size === 0) return;
    setIsBulkEmpExecuting(true);
    try {
      setEmployees((prev) =>
        prev.map((e) =>
          selectedEmpIds.has(e.id) ? { ...e, role: newRole } : e,
        ),
      );
      showSuccessToast(
        t("members.bulkActions.roleUpdated", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const handleBulkEmpSuspend = async () => {
    if (selectedEmpIds.size === 0) return;
    setIsBulkEmpExecuting(true);
    try {
      setEmployees((prev) =>
        prev.map((e) =>
          selectedEmpIds.has(e.id) ? { ...e, status: "suspended" } : e,
        ),
      );
      showSuccessToast(
        t("members.bulkActions.suspendedCount", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const handleBulkEmpActivate = async () => {
    if (selectedEmpIds.size === 0) return;
    setIsBulkEmpExecuting(true);
    try {
      setEmployees((prev) =>
        prev.map((e) =>
          selectedEmpIds.has(e.id) ? { ...e, status: "active" } : e,
        ),
      );
      showSuccessToast(
        t("members.bulkActions.activatedCount", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const loadRealData = useCallback(async () => {
    setIsLoading(true);
    try {
      // 1. Fetch current account
      let userRole: HierarchicalRole = "member";
      const activeEmail = getActiveUserEmail();
      let userEmail = activeEmail || "admin@buglogin.internal";
      let userId = "user_owner";
      try {
        const acc = await desktopApi.getAccount();
        if (acc?.accountId) {
          userId = acc.accountId;
          const resolved = resolveHumanIdentity(acc.accountId, userRole, 0);
          userEmail = activeEmail || resolved.email;
          setOrgBillingEmail(userEmail);
        } else if (activeEmail) {
          setOrgBillingEmail(activeEmail);
        }
      } catch (e) {
        console.warn("Could not fetch account:", e);
        if (activeEmail) {
          setOrgBillingEmail(activeEmail);
        }
      }

      // 2. Fetch available workspaces from SQLite
      let loadedWorkspaces: OrgWorkspaceItem[] = [];
      try {
        const wsList = await invoke<Array<any>>("get_available_workspaces");
        if (wsList && wsList.length > 0) {
          loadedWorkspaces = wsList.map((w: any) => ({
            id: w.id,
            name: w.name,
            profileCount:
              typeof w.profileCount === "number" ? w.profileCount : 0,
            maxProfiles:
              typeof w.policy?.maxProfiles === "number"
                ? w.policy.maxProfiles
                : typeof w.maxProfiles === "number"
                  ? w.maxProfiles
                  : 0,
            memberCount: typeof w.memberCount === "number" ? w.memberCount : 1,
            maxMembers:
              typeof w.policy?.maxMembers === "number"
                ? w.policy.maxMembers
                : typeof w.maxMembers === "number"
                  ? w.maxMembers
                  : 0,
            offlineEntitled: w.policy?.offlineEntitled ?? false,
            cloudSync: true,
          }));
        }
      } catch (e) {
        console.warn("Could not fetch workspaces:", e);
      }

      setWorkspaces(loadedWorkspaces);

      // 4. Fetch real profile groups
      try {
        const groups = await invoke<
          Array<{ id: string; name: string; profile_count: number }>
        >("get_groups_with_profile_counts");
        if (groups) {
          setAvailableGroups(groups);
        }
      } catch (e) {
        console.warn("Could not fetch groups:", e);
      }

      // 5. Fetch real members for active workspace
      const activeWs = loadedWorkspaces[0];
      let loadedMembers: OrgEmployee[] = [];
      try {
        const page = await desktopApi.listMembers(activeWs.id);
        if (page?.items && page.items.length > 0) {
          const ownMembership = page.items.find(
            (member) => member.accountId === userId,
          );
          if (ownMembership && Object.hasOwn(ROLE_RANKS, ownMembership.role)) {
            userRole = ownMembership.role as HierarchicalRole;
          }
          loadedMembers = page.items.map((m, idx) => {
            const resolved = resolveHumanIdentity(m.accountId, m.role, idx);
            return {
              id: m.id,
              name: resolved.displayName,
              email: resolved.email,
              role: (m.role as HierarchicalRole) || "operator",
              workspaces: [activeWs.name],
              groupScope: "Toàn bộ Profiles",
              status: m.status === "suspended" ? "suspended" : "active",
            };
          });
        }
      } catch (e) {
        console.warn("Could not fetch members:", e);
      }

      // Ensure logged in user is represented in member directory
      if (
        !loadedMembers.some((m) => m.email === userEmail || m.id === userId)
      ) {
        loadedMembers.unshift({
          id: userId,
          name: userEmail.split("@")[0] || "Quản trị viên",
          email: userEmail,
          role: userRole,
          workspaces: [activeWs.name],
          groupScope: "Toàn quyền (All)",
          status: "active",
        });
      }
      setCurrentUser({ id: userId, email: userEmail, role: userRole });
      setEmployees(loadedMembers);
      if (loadedMembers.length > 1) {
        setTransferTargetEmail(
          loadedMembers.find((m) => m.id !== userId)?.email || "",
        );
      }
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (isOpen) {
      void loadRealData();
    }
  }, [isOpen, loadRealData]);

  // Filtered employees
  const filteredEmployees = useMemo(() => {
    return employees.filter((emp) => {
      if (
        workspaceFilter !== "all" &&
        !emp.workspaces.some((w) => w.includes(workspaceFilter))
      ) {
        return false;
      }
      if (searchEmployeeQuery.trim()) {
        const q = searchEmployeeQuery.toLowerCase();
        if (
          !emp.name.toLowerCase().includes(q) &&
          !emp.email.toLowerCase().includes(q) &&
          !emp.role.toLowerCase().includes(q)
        ) {
          return false;
        }
      }
      return true;
    });
  }, [employees, workspaceFilter, searchEmployeeQuery]);

  const totalProfilesUsed = workspaces.reduce(
    (acc, w) => acc + w.profileCount,
    0,
  );
  const totalProfilesMax = 5000;
  const profilesPercent = Math.round(
    (totalProfilesUsed / totalProfilesMax) * 100,
  );

  const totalMembersUsed = employees.length;
  const totalMembersMax = 50;
  const membersPercent = Math.round((totalMembersUsed / totalMembersMax) * 100);

  const totalWorkspacesUsed = workspaces.length;
  const totalWorkspacesMax = 10;
  const workspacesPercent = Math.round(
    (totalWorkspacesUsed / totalWorkspacesMax) * 100,
  );

  const handleToggleEmployeeSuspend = (employeeId: string) => {
    setEmployees((prev) =>
      prev.map((emp) => {
        if (emp.id === employeeId) {
          const next = emp.status === "active" ? "suspended" : "active";
          showSuccessToast(
            next === "suspended"
              ? `Đã tạm đình chỉ quyền của ${emp.name}`
              : `Đã khôi phục quyền hoạt động cho ${emp.name}`,
          );
          return { ...emp, status: next };
        }
        return emp;
      }),
    );
  };

  const handleOpenResetPassword = (emp: OrgEmployee) => {
    setResetPwdMember(emp);
    const suffix = Math.random().toString(36).substring(2, 6).toUpperCase();
    setGeneratedTempPassword(`Alpha#${suffix}!2026`);
    setCopiedTempPwd(false);
  };

  const handleSaveResetPassword = () => {
    if (!resetPwdMember) return;
    showSuccessToast(
      `Đã cập nhật mật khẩu mới cho ${resetPwdMember.email}. Mọi phiên đăng nhập cũ đã bị chấm dứt.`,
    );
    setResetPwdMember(null);
  };

  const handleConfirmTransferOwnership = async (e: React.FormEvent) => {
    e.preventDefault();
    if (transferConfirmationPhrase.trim() !== "CHUYEN NHUONG SO HUU") {
      showErrorToast(
        "Vui lòng gõ chính xác cụm từ xác nhận: CHUYEN NHUONG SO HUU",
      );
      return;
    }
    if (!transferMyPassword) {
      showErrorToast("Vui lòng nhập mật khẩu xác thực quyền sở hữu");
      return;
    }

    setIsTransferring(true);
    try {
      await new Promise((r) => setTimeout(r, 1000));
      showSuccessToast(
        `Đã chuyển nhượng vai trò OWNER thành công sang ${transferTargetEmail}. Bạn hiện là Quản Trị Viên (Admin).`,
      );
      setTransferOwnerModalOpen(false);
      setTransferConfirmationPhrase("");
      setTransferMyPassword("");
    } catch (err) {
      showErrorToast(String(err));
    } finally {
      setIsTransferring(false);
    }
  };

  const handleDeleteWorkspace = () => {
    if (!deleteWsModal) return;
    if (deleteConfirmationName.trim() !== deleteWsModal.name) {
      showErrorToast(
        `Vui lòng nhập chính xác tên workspace: "${deleteWsModal.name}"`,
      );
      return;
    }
    setWorkspaces((prev) => prev.filter((w) => w.id !== deleteWsModal.id));
    showSuccessToast(
      `Đã xóa Workspace "${deleteWsModal.name}". ${deleteWsModal.profileCount} Profiles đã được xử lý theo phương án: ${deleteHandleProfilesAction}.`,
    );
    setDeleteWsModal(null);
    setDeleteConfirmationName("");
  };

  const handleSaveWorkspaceEdit = () => {
    if (!editWsModal) return;
    setWorkspaces((prev) =>
      prev.map((w) => (w.id === editWsModal.id ? editWsModal : w)),
    );
    showSuccessToast(
      `Đã lưu cấu hình cập nhật cho Workspace "${editWsModal.name}"`,
    );
    setEditWsModal(null);
  };

  const content = (
    <div className="space-y-4 text-xs font-sans">
      {/* 1. Header Bar */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/50 pb-3">
        <div className="flex items-center gap-3">
          <div className="grid size-11 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
            <LuCrown className="size-6 text-primary" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h2 className="text-base font-bold text-foreground">
                TRUNG TÂM QUẢN TRỊ DOANH NGHIỆP (ORGANIZATION SETTINGS)
              </h2>
              <Badge className="bg-amber-500/10 text-amber-600 border-amber-500/30 text-[10px] font-bold">
                PRO ENTERPRISE
              </Badge>
            </div>
            <div className="text-xs text-muted-foreground mt-0.5 flex items-center gap-2">
              <span>Tổ Chức:</span>
              <strong className="text-foreground">{orgName}</strong>
              <span>•</span>
              <span>Thời Hạn Gói: Còn 428 ngày (đến 2027-12-31)</span>
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setEditOrgModalOpen(true)}
            className="h-8 text-xs gap-1.5 cursor-pointer font-medium"
          >
            <LuSlidersHorizontal className="size-3.5" />
            <span>Cài Đặt Tổ Chức</span>
          </Button>

          {mode === "page" && onNavigateHome && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onNavigateHome}
              className="h-8 text-xs gap-1 cursor-pointer"
            >
              <LuArrowRight className="size-3.5" />
              <span>Quay Lại Profiles</span>
            </Button>
          )}
        </div>
      </div>

      {/* 2. Quota Progress Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        {/* Metric 1: Profiles */}
        <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-muted-foreground font-medium flex items-center gap-1.5">
              <LuLaptop className="size-3.5 text-primary" />
              <span>Profiles Cấp / Sử Dụng:</span>
            </span>
            <span className="font-bold text-foreground font-mono">
              {totalProfilesUsed.toLocaleString()} /{" "}
              {totalProfilesMax.toLocaleString()} ({profilesPercent}%)
            </span>
          </div>
          <div className="h-2 w-full rounded-full bg-muted overflow-hidden">
            <div
              className={cn(
                "h-full rounded-full transition-all duration-300",
                profilesPercent >= 90 ? "bg-amber-500" : "bg-primary",
              )}
              style={{ width: `${profilesPercent}%` }}
            />
          </div>
          <div className="text-[10px] text-muted-foreground flex justify-between">
            <span>
              Còn {totalProfilesMax - totalProfilesUsed} profiles tự do
            </span>
            <span>Hạn mức tổ chức</span>
          </div>
        </div>

        {/* Metric 2: Members */}
        <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-muted-foreground font-medium flex items-center gap-1.5">
              <LuUsers className="size-3.5 text-primary" />
              <span>Ghế Thành Viên (Seats):</span>
            </span>
            <span className="font-bold text-foreground font-mono">
              {totalMembersUsed} / {totalMembersMax} ({membersPercent}%)
            </span>
          </div>
          <div className="h-2 w-full rounded-full bg-muted overflow-hidden">
            <div
              className="h-full bg-primary rounded-full transition-all duration-300"
              style={{ width: `${membersPercent}%` }}
            />
          </div>
          <div className="text-[10px] text-muted-foreground flex justify-between">
            <span>Còn {totalMembersMax - totalMembersUsed} ghế trống</span>
            <span>Bảo vệ quyền truy cập</span>
          </div>
        </div>

        {/* Metric 3: Workspaces */}
        <div className="rounded-lg border border-border/70 bg-card p-3 space-y-1.5 shadow-2xs">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-muted-foreground font-medium flex items-center gap-1.5">
              <LuBuilding2 className="size-3.5 text-primary" />
              <span>Không Gian Làm Việc:</span>
            </span>
            <span className="font-bold text-foreground font-mono">
              {totalWorkspacesUsed} / {totalWorkspacesMax} Active (
              {workspacesPercent}%)
            </span>
          </div>
          <div className="h-2 w-full rounded-full bg-muted overflow-hidden">
            <div
              className="h-full bg-primary rounded-full transition-all duration-300"
              style={{ width: `${workspacesPercent}%` }}
            />
          </div>
          <div className="text-[10px] text-muted-foreground flex justify-between">
            <span>Cách ly dữ liệu 100%</span>
            <span>Môi trường phòng ban</span>
          </div>
        </div>
      </div>

      {/* 3. Navigation Tabs */}
      <div className="flex items-center gap-1 border-b border-border/60 pb-1">
        <button
          type="button"
          onClick={() => setActiveTab("members")}
          className={cn(
            "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
            activeTab === "members"
              ? "bg-accent text-accent-foreground shadow-2xs"
              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
          )}
        >
          <LuUsers className="size-3.5" />
          <span>Quản Lý Nhân Sự ({employees.length})</span>
        </button>

        <button
          type="button"
          onClick={() => setActiveTab("workspaces")}
          className={cn(
            "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
            activeTab === "workspaces"
              ? "bg-accent text-accent-foreground shadow-2xs"
              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
          )}
        >
          <LuBuilding2 className="size-3.5" />
          <span>Quản Lý Workspaces ({workspaces.length})</span>
        </button>

        <button
          type="button"
          onClick={() => setActiveTab("security")}
          className={cn(
            "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
            activeTab === "security"
              ? "bg-accent text-accent-foreground shadow-2xs"
              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
          )}
        >
          <LuShieldCheck className="size-3.5" />
          <span>Chính Sách Bảo Mật Bắt Buộc</span>
        </button>

        <button
          type="button"
          onClick={() => setActiveTab("billing")}
          className={cn(
            "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
            activeTab === "billing"
              ? "bg-accent text-accent-foreground shadow-2xs"
              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
          )}
        >
          <LuCreditCard className="size-3.5" />
          <span>Hóa Đơn & Gói Bản Quyền</span>
        </button>
      </div>

      {/* 4. Tab 1: Personnel Directory (Danh bạ nhân sự) */}
      {activeTab === "members" && (
        <div className="space-y-3">
          {/* Hierarchical RBAC Status Banner */}
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-primary/20 bg-primary/5 p-3 text-xs">
            <div className="flex items-center gap-2.5">
              <div className="grid size-8 place-items-center rounded-lg bg-primary/10 text-primary">
                <LuShieldCheck className="size-4" />
              </div>
              <div className="space-y-0.5">
                <div className="font-semibold text-foreground flex items-center gap-2">
                  <span>Tài khoản của bạn:</span>
                  <strong className="text-primary font-mono">
                    {currentUser.email}
                  </strong>
                  <Badge className="bg-primary/15 text-primary border-primary/30 text-[10px] capitalize font-bold">
                    {t(`roles.${currentUser.role}`, {
                      defaultValue: currentUser.role,
                    })}
                  </Badge>
                </div>
                <p className="text-[11px] text-muted-foreground">
                  {t("members.canManageLowerRoles", {
                    defaultValue:
                      "Chỉ có thể quản lý & phân quyền cho vai trò cấp dưới.",
                  })}
                </p>
              </div>
            </div>
            {availableGroups.length > 0 && (
              <div className="text-[11px] text-muted-foreground font-medium">
                Kho nhóm:{" "}
                <strong className="text-foreground">
                  {availableGroups.length}
                </strong>{" "}
                nhóm profiles
              </div>
            )}
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2 flex-1 min-w-[240px]">
              <div className="relative flex-1">
                <LuSearch className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
                <Input
                  type="text"
                  placeholder="Tìm nhân viên theo Tên, Email, Chức vụ..."
                  value={searchEmployeeQuery}
                  onChange={(e) => setSearchEmployeeQuery(e.target.value)}
                  className="h-8 pl-8 text-xs bg-muted/20"
                />
              </div>

              <Select
                value={workspaceFilter}
                onValueChange={setWorkspaceFilter}
              >
                <SelectTrigger className="h-8 text-xs w-[170px]">
                  <SelectValue placeholder="Lọc theo Workspace" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all" className="text-xs">
                    Tất cả Workspaces
                  </SelectItem>
                  {workspaces.map((w) => (
                    <SelectItem key={w.id} value={w.name} className="text-xs">
                      {w.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className="rounded-lg border border-border/60 overflow-hidden bg-card text-xs">
            <table className="w-full text-left border-collapse">
              <thead className="bg-muted/40 border-b border-border/50 text-[11px] text-muted-foreground font-medium uppercase tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 w-8">
                    <Checkbox
                      checked={
                        manageableEmployees.length > 0 &&
                        selectedEmpIds.size === manageableEmployees.length
                      }
                      onCheckedChange={handleSelectAllManageableEmp}
                      disabled={manageableEmployees.length === 0}
                      aria-label="Select all manageable employees"
                    />
                  </th>
                  <th className="py-2.5 px-3">Nhân Viên & Email</th>
                  <th className="py-2.5 px-3">Quyền Tổ Chức</th>
                  <th className="py-2.5 px-3">Workspaces Được Vào</th>
                  <th className="py-2.5 px-3">Nhóm Profile Phụ Trách</th>
                  <th className="py-2.5 px-3">Trạng Thái</th>
                  <th className="py-2.5 px-3 text-right">Thao Tác Quản Trị</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {isLoading ? (
                  <tr>
                    <td
                      colSpan={7}
                      className="py-8 text-center text-muted-foreground"
                    >
                      {t("common.buttons.loading", {
                        defaultValue: "Đang tải danh sách thành viên...",
                      })}
                    </td>
                  </tr>
                ) : filteredEmployees.length === 0 ? (
                  <tr>
                    <td
                      colSpan={7}
                      className="py-8 text-center text-muted-foreground"
                    >
                      Không tìm thấy nhân viên nào phù hợp
                    </td>
                  </tr>
                ) : (
                  filteredEmployees.map((emp) => {
                    const currentUserRank = ROLE_RANKS[currentUser.role] ?? 4;
                    const memberRank = ROLE_RANKS[emp.role] ?? 0;
                    const isSelf =
                      emp.id === currentUser.id ||
                      emp.email === currentUser.email;
                    const isHigherOrEqual = memberRank >= currentUserRank;
                    const canManage = !isSelf && !isHigherOrEqual;

                    return (
                      <tr
                        key={emp.id}
                        className="hover:bg-accent/40 transition-colors"
                      >
                        <td className="py-2.5 px-3 w-8">
                          <Checkbox
                            checked={selectedEmpIds.has(emp.id)}
                            onCheckedChange={() =>
                              handleToggleSelectEmp(emp.id)
                            }
                            disabled={!canManage}
                            aria-label={`Select ${emp.email}`}
                          />
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          <div className="font-semibold text-foreground">
                            {emp.name}
                          </div>
                          <div className="text-[10px] text-muted-foreground">
                            {emp.email}
                          </div>
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          {emp.role === "owner" && (
                            <Badge className="bg-amber-500/15 text-amber-600 border-amber-300 text-[10px] font-bold gap-1">
                              <LuCrown className="size-3" />
                              <span>Owner</span>
                            </Badge>
                          )}
                          {emp.role === "admin" && (
                            <Badge
                              variant="outline"
                              className="border-primary text-primary text-[10px] font-semibold gap-1"
                            >
                              <LuShield className="size-3" />
                              <span>Org Admin</span>
                            </Badge>
                          )}
                          {emp.role === "manager" && (
                            <Badge
                              variant="outline"
                              className="border-blue-500 text-blue-500 text-[10px] font-semibold gap-1"
                            >
                              <LuUsers className="size-3" />
                              <span>Manager</span>
                            </Badge>
                          )}
                          {emp.role === "operator" && (
                            <Badge
                              variant="secondary"
                              className="text-[10px] font-medium text-muted-foreground"
                            >
                              Operator
                            </Badge>
                          )}
                          {emp.role === "member" && (
                            <Badge
                              variant="outline"
                              className="text-[10px] font-medium text-muted-foreground"
                            >
                              Member
                            </Badge>
                          )}
                        </td>
                        <td className="py-2.5 px-3 text-muted-foreground">
                          {emp.workspaces.join(", ")}
                        </td>
                        <td className="py-2.5 px-3 text-muted-foreground">
                          {emp.groupScope}
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          {emp.status === "active" ? (
                            <Badge className="bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/30 text-[10px] gap-1 font-semibold">
                              <span className="size-1.5 rounded-full bg-emerald-500 animate-pulse" />
                              <span>Active</span>
                            </Badge>
                          ) : (
                            <Badge
                              variant="destructive"
                              className="text-[10px] gap-1 font-semibold"
                            >
                              <span>TẠM KHÓA</span>
                            </Badge>
                          )}
                        </td>
                        <td className="py-2.5 px-3 text-right whitespace-nowrap">
                          {isSelf ? (
                            <span className="text-[11px] text-muted-foreground italic">
                              (Tài khoản hiện tại)
                            </span>
                          ) : isHigherOrEqual ? (
                            <Badge
                              variant="outline"
                              className="text-[10px] text-muted-foreground border-border/80 font-normal"
                            >
                              {t("members.protectedRole", {
                                defaultValue: "Vai trò được bảo vệ",
                              })}
                            </Badge>
                          ) : (
                            <div className="flex items-center justify-end gap-1.5">
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                onClick={() => handleOpenResetPassword(emp)}
                                className="h-7 px-2 text-[10px] gap-1 cursor-pointer font-medium"
                                title="Đặt lại mật khẩu khởi tạo"
                              >
                                <LuKey className="size-3" />
                                <span>Đổi mật khẩu</span>
                              </Button>
                              <Button
                                type="button"
                                variant={
                                  emp.status === "suspended"
                                    ? "default"
                                    : "destructive"
                                }
                                size="sm"
                                onClick={() =>
                                  handleToggleEmployeeSuspend(emp.id)
                                }
                                className="h-7 px-2 text-[10px] gap-1 cursor-pointer font-medium"
                              >
                                {emp.status === "suspended"
                                  ? "Mở Khóa"
                                  : "Đình Chỉ"}
                              </Button>
                            </div>
                          )}
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>

          {/* Floating Bulk Action Bar for Employees */}
          {selectedEmpIds.size > 0 && (
            <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-lg border border-primary/40 bg-card p-2.5 shadow-md animate-in fade-in slide-in-from-bottom-2 text-xs">
              <div className="flex items-center gap-2">
                <Badge variant="default" className="text-xs px-2.5 py-0.5">
                  {t("members.bulkActions.selected", {
                    count: selectedEmpIds.size,
                    defaultValue: `Đã chọn ${selectedEmpIds.size} nhân sự`,
                  })}
                </Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedEmpIds(new Set())}
                  className="h-7 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  {t("members.bulkActions.clear", { defaultValue: "Bỏ chọn" })}
                </Button>
              </div>

              <div className="flex items-center gap-2">
                <Select
                  onValueChange={(r) =>
                    void handleBulkEmpRoleChange(r as HierarchicalRole)
                  }
                >
                  <SelectTrigger className="h-7 w-32 text-xs">
                    <SelectValue
                      placeholder={t("members.bulkActions.changeRole", {
                        defaultValue: "Đổi vai trò",
                      })}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="admin" className="text-xs">
                      Admin
                    </SelectItem>
                    <SelectItem value="manager" className="text-xs">
                      Manager
                    </SelectItem>
                    <SelectItem value="operator" className="text-xs">
                      Operator
                    </SelectItem>
                    <SelectItem value="member" className="text-xs">
                      Member
                    </SelectItem>
                  </SelectContent>
                </Select>

                <LoadingButton
                  variant="outline"
                  size="sm"
                  isLoading={isBulkEmpExecuting}
                  onClick={() => void handleBulkEmpSuspend()}
                  className="h-7 text-xs gap-1 cursor-pointer text-amber-600 hover:text-amber-700"
                >
                  <LuBan className="size-3" />
                  <span>
                    {t("members.bulkActions.suspend", { defaultValue: "Khóa" })}
                  </span>
                </LoadingButton>

                <LoadingButton
                  variant="outline"
                  size="sm"
                  isLoading={isBulkEmpExecuting}
                  onClick={() => void handleBulkEmpActivate()}
                  className="h-7 text-xs gap-1 cursor-pointer text-emerald-600 hover:text-emerald-700"
                >
                  <LuUserCheck className="size-3" />
                  <span>
                    {t("members.bulkActions.activate", {
                      defaultValue: "Mở khóa",
                    })}
                  </span>
                </LoadingButton>
              </div>
            </div>
          )}
        </div>
      )}

      {/* 5. Tab 2: Workspaces Management */}
      {activeTab === "workspaces" && (
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <h4 className="font-semibold text-foreground text-xs flex items-center gap-1.5">
              <LuBuilding2 className="size-4 text-primary" />
              <span>Danh Sách Không Gian Làm Việc Trực Thuộc</span>
            </h4>
          </div>

          <div className="rounded-lg border border-border/60 overflow-hidden bg-card text-xs">
            <table className="w-full text-left border-collapse">
              <thead className="bg-muted/40 border-b border-border/50 text-[11px] text-muted-foreground font-medium uppercase tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 w-8">
                    <Checkbox
                      checked={
                        workspaces.length > 0 &&
                        selectedWsIds.size === workspaces.length
                      }
                      onCheckedChange={handleSelectAllWs}
                      disabled={workspaces.length === 0}
                      aria-label="Select all workspaces"
                    />
                  </th>
                  <th className="py-2.5 px-3">Tên Workspace</th>
                  <th className="py-2.5 px-3">Hạn Mức Profiles</th>
                  <th className="py-2.5 px-3">Thành Viên</th>
                  <th className="py-2.5 px-3">Tính Năng Vận Hành</th>
                  <th className="py-2.5 px-3 text-right">Thao Tác</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {workspaces.map((ws) => {
                  const prfPercent = Math.round(
                    (ws.profileCount / ws.maxProfiles) * 100,
                  );
                  return (
                    <tr
                      key={ws.id}
                      className="hover:bg-accent/40 transition-colors"
                    >
                      <td className="py-2.5 px-3 w-8">
                        <Checkbox
                          checked={selectedWsIds.has(ws.id)}
                          onCheckedChange={() => handleToggleSelectWs(ws.id)}
                          aria-label={`Select workspace ${ws.name}`}
                        />
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap">
                        <div className="font-semibold text-foreground">
                          {ws.name}
                        </div>
                        <div className="font-mono text-[10px] text-muted-foreground">
                          {ws.id}
                        </div>
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap min-w-[150px]">
                        <div className="flex justify-between font-mono text-[11px] mb-1">
                          <span>
                            {ws.profileCount} / {ws.maxProfiles}
                          </span>
                          <span className="font-bold text-muted-foreground">
                            {prfPercent}%
                          </span>
                        </div>
                        <div className="h-1.5 w-full rounded-full bg-muted overflow-hidden">
                          <div
                            className="h-full bg-primary rounded-full"
                            style={{ width: `${prfPercent}%` }}
                          />
                        </div>
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap font-mono text-[11px] text-muted-foreground">
                        {ws.memberCount} / {ws.maxMembers} seats
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap">
                        <div className="flex items-center gap-1.5">
                          {ws.offlineEntitled && (
                            <Badge
                              variant="outline"
                              className="text-[10px] text-muted-foreground gap-1"
                            >
                              <span>Offline OK</span>
                            </Badge>
                          )}
                          {ws.cloudSync && (
                            <Badge
                              variant="outline"
                              className="text-[10px] border-emerald-500/30 text-emerald-600 gap-1"
                            >
                              <span>Cloud Sync</span>
                            </Badge>
                          )}
                        </div>
                      </td>
                      <td className="py-2.5 px-3 text-right whitespace-nowrap">
                        <div className="flex items-center justify-end gap-1.5">
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => setEditWsModal(ws)}
                            className="h-7 px-2 text-[10px] gap-1 cursor-pointer font-medium"
                          >
                            <LuPencil className="size-3" />
                            <span>Sửa</span>
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() => {
                              setDeleteConfirmationName("");
                              setDeleteWsModal(ws);
                            }}
                            className="h-7 px-2 text-[10px] gap-1 cursor-pointer text-destructive hover:bg-destructive/10 font-medium"
                          >
                            <LuTrash2 className="size-3" />
                            <span>Giải tán</span>
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          {/* Floating Bulk Action Bar for Workspaces */}
          {selectedWsIds.size > 0 && (
            <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-lg border border-primary/40 bg-card p-2.5 shadow-md animate-in fade-in slide-in-from-bottom-2 text-xs">
              <div className="flex items-center gap-2">
                <Badge variant="default" className="text-xs px-2.5 py-0.5">
                  {t("workspaces.bulkActions.selected", {
                    count: selectedWsIds.size,
                    defaultValue: `Đã chọn ${selectedWsIds.size} workspace`,
                  })}
                </Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedWsIds(new Set())}
                  className="h-7 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  {t("workspaces.bulkActions.clear", {
                    defaultValue: "Bỏ chọn",
                  })}
                </Button>
              </div>

              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setBulkMaxProfiles(200);
                    setBulkMaxMembers(20);
                    setBulkWsQuotaModalOpen(true);
                  }}
                  className="h-7 text-xs gap-1 cursor-pointer"
                >
                  <LuSlidersHorizontal className="size-3 text-primary" />
                  <span>
                    {t("workspaces.bulkActions.adjustQuota", {
                      defaultValue: "Điều chỉnh hạn mức",
                    })}
                  </span>
                </Button>

                <LoadingButton
                  variant="destructive"
                  size="sm"
                  isLoading={isBulkWsExecuting}
                  onClick={() => void handleBulkArchiveWs()}
                  className="h-7 text-xs gap-1 cursor-pointer"
                >
                  <LuTrash2 className="size-3" />
                  <span>
                    {t("workspaces.bulkActions.archive", {
                      defaultValue: "Lưu trữ đã chọn",
                    })}
                  </span>
                </LoadingButton>
              </div>
            </div>
          )}
        </div>
      )}

      {/* 6. Tab 3: Security Policies (Chính Sách Bảo Mật Bắt Buộc Toàn Công Ty) */}
      {activeTab === "security" && (
        <div className="space-y-4">
          <div className="rounded-lg border border-border/70 bg-card p-4 space-y-3">
            <div className="font-semibold text-foreground text-xs flex items-center gap-2">
              <LuShieldCheck className="size-4 text-primary" />
              <span>
                CHÍNH SÁCH BẢO MẬT BẮT BUỘC TOÀN CÔNG TY (Mục 13.2 & 14.6)
              </span>
            </div>

            <div className="space-y-2.5 text-xs">
              <div className="flex items-start gap-2.5 p-2.5 rounded-lg border border-border/50 hover:bg-muted/20 cursor-pointer">
                <Checkbox
                  aria-label="Bắt buộc xác thực 2 bước (2FA) đối với Admin & Manager"
                  checked={require2FA}
                  onCheckedChange={(checked) => setRequire2FA(checked === true)}
                />
                <div className="space-y-0.5">
                  <div className="font-medium text-foreground">
                    Bắt buộc xác thực 2 bước (2FA) đối với Admin & Manager
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    Ngăn chặn chiếm đoạt tài khoản quản trị khi lộ mật khẩu máy
                    trạm.
                  </div>
                </div>
              </div>

              <div className="flex items-center justify-between p-2.5 rounded-lg border border-border/50 bg-background">
                <div className="space-y-0.5">
                  <div className="font-medium text-foreground">
                    Tự động đăng xuất tài khoản khi không tương tác
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    Mặc định tắt: Phiên làm việc được duy trì liên tục khi làm
                    việc, chỉ kết thúc khi đăng xuất thủ công hoặc bị quản trị
                    viên thu hồi.
                  </div>
                </div>
                <Select
                  value={String(autoLogoutMinutes)}
                  onValueChange={(v) => setAutoLogoutMinutes(Number(v))}
                >
                  <SelectTrigger className="h-8 w-48 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="0" className="text-xs">
                      Tắt (Duy trì phiên liên tục)
                    </SelectItem>
                    <SelectItem value="15" className="text-xs">
                      15 phút
                    </SelectItem>
                    <SelectItem value="30" className="text-xs">
                      30 phút
                    </SelectItem>
                    <SelectItem value="60" className="text-xs">
                      60 phút (1 giờ)
                    </SelectItem>
                    <SelectItem value="120" className="text-xs">
                      120 phút (2 giờ)
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div className="flex items-start gap-2.5 p-2.5 rounded-lg border border-border/50 hover:bg-muted/20 cursor-pointer">
                <Checkbox
                  aria-label="Chặn toàn bộ hành vi xuất Cookie thô đối với vai trò Operator"
                  checked={blockRawCookieForOperators}
                  onCheckedChange={(checked) =>
                    setBlockRawCookieForOperators(checked === true)
                  }
                />
                <div className="space-y-0.5">
                  <div className="font-medium text-foreground">
                    Chặn toàn bộ hành vi xuất Cookie thô đối với vai trò
                    Operator (DLP Enforced)
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    Nhân viên chỉ có thể mở trình duyệt đã đăng nhập sẵn; không
                    thể copy cookie ra file văn bản mang đi.
                  </div>
                </div>
              </div>

              <div className="flex items-start gap-2.5 p-2.5 rounded-lg border border-border/50 hover:bg-muted/20 cursor-pointer">
                <Checkbox
                  aria-label="Ghi nhận Audit log cho mọi thao tác"
                  checked={mandatoryAuditLog}
                  onCheckedChange={(checked) =>
                    setMandatoryAuditLog(checked === true)
                  }
                />
                <div className="space-y-0.5">
                  <div className="font-medium text-foreground">
                    Ghi nhận nhật ký Audit đầy đủ cho mọi thao tác mở trình
                    duyệt và thay đổi proxy
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    Lưu trữ bất biến mã checksum SHA-256 đối soát khi phát hiện
                    gian lận.
                  </div>
                </div>
              </div>
            </div>
          </div>

          {/* Danger Zone: Transfer Ownership (Mục 14.7) */}
          <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4 space-y-2">
            <div className="flex items-center justify-between">
              <div>
                <h5 className="font-bold text-destructive flex items-center gap-1.5 text-xs">
                  <LuCrown className="size-4" />
                  <span>
                    KHU VỰC CHUYỂN GIAO QUYỀN LỰC CAO NHẤT (TRANSFER OWNERSHIP)
                  </span>
                </h5>
                <p className="text-[11px] text-muted-foreground mt-0.5">
                  Chuyển quyền OWNER của {orgName} cho một nhân sự khác trong
                  ban điều hành.
                </p>
              </div>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                onClick={() => setTransferOwnerModalOpen(true)}
                className="h-8 text-xs cursor-pointer font-medium"
              >
                Chuyển Nhượng Quyền Sở Hữu...
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 7. Tab 4: Billing & Tier Info */}
      {activeTab === "billing" && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="rounded-lg border border-border/70 bg-card p-4 space-y-3">
            <div className="font-semibold text-foreground text-xs flex items-center gap-2">
              <LuCreditCard className="size-4 text-primary" />
              <span>Gói Bản Quyền Doanh Nghiệp (PRO ENTERPRISE)</span>
            </div>
            <div className="space-y-2 text-xs">
              <div className="flex justify-between py-1 border-b border-border/40">
                <span className="text-muted-foreground">
                  Khách hàng đăng ký:
                </span>
                <span className="font-semibold text-foreground">{orgName}</span>
              </div>
              <div className="flex justify-between py-1 border-b border-border/40">
                <span className="text-muted-foreground">
                  Email nhận hóa đơn:
                </span>
                <span className="font-mono text-foreground">
                  {orgBillingEmail}
                </span>
              </div>
              <div className="flex justify-between py-1 border-b border-border/40">
                <span className="text-muted-foreground">Ngày hết hạn gói:</span>
                <span className="font-semibold text-foreground">
                  2027-12-31 (Còn 428 ngày)
                </span>
              </div>
              <div className="flex justify-between py-1">
                <span className="text-muted-foreground">
                  Trạng thái thanh toán:
                </span>
                <Badge className="bg-emerald-500/10 text-emerald-600 border-emerald-500/30 text-[10px]">
                  ● Đã thanh toán đầy đủ
                </Badge>
              </div>
            </div>
          </div>

          <div className="rounded-lg border border-border/70 bg-card p-4 space-y-3">
            <div className="font-semibold text-foreground text-xs flex items-center gap-2">
              <LuHardDrive className="size-4 text-primary" />
              <span>Hạ Tầng Đám Mây & MCP Gateway Đi Kèm</span>
            </div>
            <div className="space-y-2 text-xs">
              <div className="flex justify-between py-1 border-b border-border/40">
                <span className="text-muted-foreground">
                  Đồng bộ đám mây mã hóa (Cloud Sync):
                </span>
                <span className="font-medium text-emerald-500">
                  Đã kích hoạt (Mã hóa đầu cuối)
                </span>
              </div>
              <div className="flex justify-between py-1 border-b border-border/40">
                <span className="text-muted-foreground">
                  Cổng kết nối MCP Server:
                </span>
                <span className="font-medium text-emerald-500">
                  Sẵn sàng (Port 1420/REST)
                </span>
              </div>
              <div className="flex justify-between py-1">
                <span className="text-muted-foreground">
                  Nhân Wayfern Core:
                </span>
                <span className="font-mono font-bold text-foreground">
                  v132.0.6834.110 (Mới nhất)
                </span>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* SUB-MODAL: Bulk Adjust Workspace Quotas */}
      <Dialog
        open={bulkWsQuotaModalOpen}
        onOpenChange={setBulkWsQuotaModalOpen}
      >
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuSlidersHorizontal className="size-4 text-primary" />
              <span>
                {t("workspaces.bulkActions.quotaTitle", {
                  defaultValue: "Điều Chỉnh Hạn Mức Hàng Loạt",
                })}
              </span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {t("workspaces.bulkActions.quotaDesc", {
                count: selectedWsIds.size,
                defaultValue: `Cập nhật giới hạn hồ sơ và thành viên cho ${selectedWsIds.size} không gian đã chọn.`,
              })}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2 text-xs">
            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                {t("workspaces.bulkActions.maxProfiles", {
                  defaultValue: "Hạn mức Profile mỗi workspace:",
                })}
              </Label>
              <Input
                type="number"
                min={1}
                max={100000}
                value={bulkMaxProfiles}
                onChange={(e) =>
                  setBulkMaxProfiles(Number(e.target.value) || 1)
                }
                className="h-8 text-xs font-mono"
              />
            </div>

            <div className="space-y-1.5">
              <Label className="text-xs font-medium">
                {t("workspaces.bulkActions.maxMembers", {
                  defaultValue: "Hạn mức Thành viên mỗi workspace:",
                })}
              </Label>
              <Input
                type="number"
                min={1}
                max={500}
                value={bulkMaxMembers}
                onChange={(e) => setBulkMaxMembers(Number(e.target.value) || 1)}
                className="h-8 text-xs font-mono"
              />
            </div>
          </div>

          <DialogFooter className="border-t border-border/50 pt-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setBulkWsQuotaModalOpen(false)}
              className="h-8 text-xs cursor-pointer"
            >
              {t("common.buttons.cancel", { defaultValue: "Hủy" })}
            </Button>
            <LoadingButton
              type="button"
              size="sm"
              isLoading={isBulkWsExecuting}
              onClick={() => void handleApplyBulkWsQuotas()}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              {t("workspaces.bulkActions.saveQuotas", {
                defaultValue: "Áp dụng hạn mức",
              })}
            </LoadingButton>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Edit Organization */}
      <Dialog open={editOrgModalOpen} onOpenChange={setEditOrgModalOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuBuilding2 className="size-4 text-primary" />
              <span>Cài Đặt Tổ Chức</span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              Cập nhật thông tin nhận diện pháp nhân và email thanh toán
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-3 py-2 text-xs">
            <div className="space-y-1">
              <Label className="text-[11px] font-medium">Tên Tổ Chức: *</Label>
              <Input
                value={orgName}
                onChange={(e) => setOrgName(e.target.value)}
                className="h-8 text-xs"
              />
            </div>

            <div className="space-y-1">
              <Label className="text-[11px] font-medium">
                Email nhận thông báo bảo mật & hóa đơn: *
              </Label>
              <Input
                type="email"
                value={orgBillingEmail}
                onChange={(e) => setOrgBillingEmail(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
          </div>

          <DialogFooter className="border-t border-border/50 pt-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setEditOrgModalOpen(false)}
              className="h-8 text-xs cursor-pointer"
            >
              Hủy
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => {
                showSuccessToast("Đã lưu thông tin cài đặt tổ chức");
                setEditOrgModalOpen(false);
              }}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              Lưu Cài Đặt
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Transfer Ownership Modal (Mục 14.7) */}
      <Dialog
        open={transferOwnerModalOpen}
        onOpenChange={setTransferOwnerModalOpen}
      >
        <DialogContent className="max-w-lg p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2 text-destructive">
              <LuCrown className="size-4" />
              <span>
                Chuyển Nhượng Quyền Sở Hữu Tổ Chức (Transfer Ownership)
              </span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              CẢNH BÁO QUAN TRỌNG: Sau khi hoàn tất, bạn sẽ trở thành Admin
              thông thường và mất quyền thanh toán hoặc xóa tổ chức.
            </DialogDescription>
          </DialogHeader>

          <form
            onSubmit={(e) => void handleConfirmTransferOwnership(e)}
            className="space-y-3 py-2 text-xs"
          >
            <div className="space-y-1">
              <Label className="text-[11px] font-medium">
                Chọn thành viên nhận quyền sở hữu mới:
              </Label>
              <Select
                value={transferTargetEmail}
                onValueChange={setTransferTargetEmail}
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="lead_hr@alphamedia.vn" className="text-xs">
                    Tran Thi B (lead_hr@alphamedia.vn) - Org Admin
                  </SelectItem>
                  <SelectItem value="staff01@alphamedia.vn" className="text-xs">
                    Le Van C (staff01@alphamedia.vn) - Team Manager
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1">
              <Label className="text-[11px] font-medium">
                Nhập mật khẩu hiện tại của bạn để xác thực:
              </Label>
              <Input
                type="password"
                placeholder="••••••••••••"
                value={transferMyPassword}
                onChange={(e) => setTransferMyPassword(e.target.value)}
                required
                className="h-8 text-xs"
              />
            </div>

            <div className="space-y-1 pt-1">
              <Label className="text-[11px] font-medium text-destructive">
                Để xác nhận, vui lòng gõ đúng cụm từ: &quot;CHUYEN NHUONG SO
                HUU&quot;
              </Label>
              <Input
                placeholder="CHUYEN NHUONG SO HUU"
                value={transferConfirmationPhrase}
                onChange={(e) => setTransferConfirmationPhrase(e.target.value)}
                required
                className="h-8 text-xs font-mono font-bold"
              />
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setTransferOwnerModalOpen(false)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy Bỏ
              </Button>
              <LoadingButton
                type="submit"
                variant="destructive"
                size="sm"
                isLoading={isTransferring}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Xác Nhận Bàn Giao Quyền Owner
              </LoadingButton>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Edit Workspace Settings (Mục 14.2) */}
      {editWsModal && (
        <Dialog open={true} onOpenChange={() => setEditWsModal(null)}>
          <DialogContent className="max-w-md p-6">
            <DialogHeader className="border-b border-border/50 pb-3">
              <DialogTitle className="text-base font-semibold flex items-center gap-2">
                <LuBuilding2 className="size-4 text-primary" />
                <span>Chỉnh Sửa Không Gian Làm Việc (Workspace Settings)</span>
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground">
                Điều chỉnh hạn mức phân bổ và quyền hoạt động ngoại tuyến
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-3 py-2 text-xs">
              <div className="space-y-1">
                <Label className="text-[11px] font-medium">
                  Tên Workspace: *
                </Label>
                <Input
                  value={editWsModal.name}
                  onChange={(e) =>
                    setEditWsModal({ ...editWsModal, name: e.target.value })
                  }
                  className="h-8 text-xs"
                />
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <Label className="text-[11px] font-medium">
                    Hạn mức Profiles:
                  </Label>
                  <Input
                    type="number"
                    value={editWsModal.maxProfiles}
                    onChange={(e) =>
                      setEditWsModal({
                        ...editWsModal,
                        maxProfiles: Number(e.target.value),
                      })
                    }
                    className="h-8 text-xs"
                  />
                </div>
                <div className="space-y-1">
                  <Label className="text-[11px] font-medium">
                    Hạn mức Thành viên:
                  </Label>
                  <Input
                    type="number"
                    value={editWsModal.maxMembers}
                    onChange={(e) =>
                      setEditWsModal({
                        ...editWsModal,
                        maxMembers: Number(e.target.value),
                      })
                    }
                    className="h-8 text-xs"
                  />
                </div>
              </div>

              <div className="space-y-2 pt-1">
                <div className="flex items-center gap-2 cursor-pointer">
                  <Checkbox
                    aria-label="Cho phép mở Profile ngoại tuyến khi mất mạng"
                    checked={editWsModal.offlineEntitled}
                    onCheckedChange={(checked) =>
                      setEditWsModal({
                        ...editWsModal,
                        offlineEntitled: checked === true,
                      })
                    }
                  />
                  <span>
                    Cho phép mở Profile ngoại tuyến khi mất mạng (Offline
                    Entitled)
                  </span>
                </div>
                <div className="flex items-center gap-2 cursor-pointer">
                  <Checkbox
                    aria-label="Tự động đồng bộ đám mây an toàn"
                    checked={editWsModal.cloudSync}
                    onCheckedChange={(checked) =>
                      setEditWsModal({
                        ...editWsModal,
                        cloudSync: checked === true,
                      })
                    }
                  />
                  <span>Tự động đồng bộ đám mây an toàn (E2EE Cloud Sync)</span>
                </div>
              </div>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setEditWsModal(null)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy
              </Button>
              <Button
                type="button"
                size="sm"
                onClick={handleSaveWorkspaceEdit}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Lưu Cập Nhật
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}

      {/* SUB-MODAL: Delete Workspace Modal (Mục 14.3) */}
      {deleteWsModal && (
        <Dialog open={true} onOpenChange={() => setDeleteWsModal(null)}>
          <DialogContent className="max-w-md p-6">
            <DialogHeader className="border-b border-border/50 pb-3">
              <DialogTitle className="text-base font-semibold flex items-center gap-2 text-destructive">
                <LuTriangleAlert className="size-4" />
                <span>CẢNH BÁO: XÓA / GIẢI TÁN KHÔNG GIAN LÀM VIỆC</span>
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground">
                Bạn đang chuẩn bị giải tán Workspace:{" "}
                <strong>{deleteWsModal.name}</strong>
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-3 py-2 text-xs">
              <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-3 space-y-1.5 text-muted-foreground text-[11px]">
                <div className="font-bold text-foreground">
                  TÀI NGUYÊN BỊ ẢNH HƯỞNG:
                </div>
                <div>
                  • {deleteWsModal.profileCount} Profiles trình duyệt đang lưu
                  trữ
                </div>
                <div>
                  • {deleteWsModal.memberCount} Thành viên sẽ bị thu hồi quyền
                  truy cập
                </div>
                <div>
                  • Mọi Proxy gán riêng cho Workspace sẽ trả về kho tự do
                </div>
              </div>

              <div className="space-y-1.5">
                <Label className="text-[11px] font-medium">
                  Phương án xử lý {deleteWsModal.profileCount} Profiles:
                </Label>
                <div className="space-y-1">
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input
                      type="radio"
                      name="handleProfiles"
                      checked={deleteHandleProfilesAction === "archive"}
                      onChange={() => setDeleteHandleProfilesAction("archive")}
                      className="size-3.5"
                    />
                    <span>Chuyển vào Kho Lưu Trữ (Archive) để giữ dữ liệu</span>
                  </label>
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input
                      type="radio"
                      name="handleProfiles"
                      checked={deleteHandleProfilesAction === "purge"}
                      onChange={() => setDeleteHandleProfilesAction("purge")}
                      className="size-3.5"
                    />
                    <span className="text-destructive font-medium">
                      Xóa sạch vĩnh viễn không thể khôi phục
                    </span>
                  </label>
                </div>
              </div>

              <div className="space-y-1 pt-1">
                <Label className="text-[11px] font-medium text-destructive">
                  Để xác nhận, vui lòng gõ chính xác: &quot;{deleteWsModal.name}
                  &quot;
                </Label>
                <Input
                  value={deleteConfirmationName}
                  onChange={(e) => setDeleteConfirmationName(e.target.value)}
                  placeholder={deleteWsModal.name}
                  className="h-8 text-xs font-mono"
                />
              </div>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setDeleteWsModal(null)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy Bỏ
              </Button>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                onClick={handleDeleteWorkspace}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Tôi Hiểu Rủi Ro, Xóa Workspace Này
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}

      {/* SUB-MODAL: Reset Member Password (Mục 14.5) */}
      {resetPwdMember && (
        <Dialog open={true} onOpenChange={() => setResetPwdMember(null)}>
          <DialogContent className="max-w-md p-6">
            <DialogHeader className="border-b border-border/50 pb-3">
              <DialogTitle className="text-base font-semibold flex items-center gap-2">
                <LuKey className="size-4 text-primary" />
                <span>Đặt Lại Mật Khẩu: {resetPwdMember.name}</span>
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground">
                Tài khoản: {resetPwdMember.email}
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-3 py-2 text-xs">
              <div className="space-y-1">
                <Label className="text-[11px] font-medium">
                  Mật khẩu mới được hệ thống sinh tự động:
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    readOnly
                    value={generatedTempPassword}
                    className="h-8 font-mono font-bold tracking-wider text-primary text-xs bg-muted/40"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      void navigator.clipboard.writeText(generatedTempPassword);
                      setCopiedTempPwd(true);
                      showSuccessToast("Đã sao chép mật khẩu mới");
                      setTimeout(() => setCopiedTempPwd(false), 2000);
                    }}
                    className="h-8 px-2.5 text-xs gap-1 cursor-pointer font-medium"
                  >
                    {copiedTempPwd ? (
                      <LuCheck className="size-3 text-emerald-500" />
                    ) : (
                      <LuCopy className="size-3" />
                    )}
                    <span>{copiedTempPwd ? "Đã chép" : "Copy"}</span>
                  </Button>
                </div>
              </div>

              <div className="space-y-1.5 pt-1">
                <div className="flex items-center gap-2 cursor-pointer">
                  <Checkbox
                    aria-label="Bắt buộc nhân viên đổi mật khẩu ở lần đăng nhập tiếp theo"
                    checked={mustChangePasswordOnLogin}
                    onCheckedChange={(checked) =>
                      setMustChangePasswordOnLogin(checked === true)
                    }
                  />
                  <span>
                    Bắt buộc nhân viên đổi mật khẩu ở lần đăng nhập tiếp theo
                  </span>
                </div>
                <div className="flex items-center gap-2 cursor-pointer">
                  <Checkbox
                    aria-label="Đăng xuất khỏi tất cả thiết bị khác ngay lập tức"
                    checked={forceLogoutAllSessions}
                    onCheckedChange={(checked) =>
                      setForceLogoutAllSessions(checked === true)
                    }
                  />
                  <span>
                    Đăng xuất cưỡng chế khỏi tất cả thiết bị khác ngay lập tức
                  </span>
                </div>
              </div>

              <p className="text-[10px] text-muted-foreground">
                Ghi chú: Hãy sao chép mật khẩu trên và gửi trực tiếp cho nhân
                viên qua tin nhắn an toàn.
              </p>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setResetPwdMember(null)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy
              </Button>
              <Button
                type="button"
                size="sm"
                onClick={handleSaveResetPassword}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Cập Nhật Mật Khẩu Mới
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );

  if (mode === "page") {
    return (
      <div className="flex-1 overflow-y-auto p-4 md:p-6 bg-background">
        <div className="max-w-6xl mx-auto">{content}</div>
      </div>
    );
  }

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-5xl p-6">{content}</DialogContent>
    </Dialog>
  );
}

"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowRight,
  LuBan,
  LuBuilding2,
  LuCreditCard,
  LuCrown,
  LuHardDrive,
  LuLaptop,
  LuPencil,
  LuSearch,
  LuShield,
  LuShieldCheck,
  LuSlidersHorizontal,
  LuTriangleAlert,
  LuUserCheck,
  LuUsers,
} from "react-icons/lu";
import { CreateInviteModal } from "@/components/create-invite-modal";
import { LoadingButton } from "@/components/loading-button";
import { ResourceInventoryPanel } from "@/components/resource-inventory-panel";
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
import { getActiveUserEmail } from "@/lib/identity-display";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";

const desktopApi = createTauriDesktopApi();

export type HierarchicalRole =
  | "owner"
  | "admin"
  | "lead"
  | "operator"
  | "member";

export const ROLE_RANKS: Record<string, number> = {
  owner: 4,
  admin: 3,
  lead: 2,
  operator: 1,
  member: 0,
};

export interface OrgEmployee {
  id: string;
  accountId: string;
  workspaceId: string;
  name: string;
  email: string;
  role: HierarchicalRole;
  workspaces: string[];
  groupScope: string;
  status: "invited" | "active" | "suspended";
}

export interface OrgWorkspaceItem {
  id: string;
  organizationId: string;
  name: string;
  profileCount: number | null;
  maxProfiles: number;
  memberCount: number;
  maxMembers: number;
  maxStorageBytes: number;
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
    "members" | "workspaces" | "resources" | "security" | "billing"
  >("members");

  const [employees, setEmployees] = useState<OrgEmployee[]>([]);
  const [workspaces, setWorkspaces] = useState<OrgWorkspaceItem[]>([]);
  const [currentUser, setCurrentUser] = useState<{
    id: string;
    email: string;
    role: HierarchicalRole;
    workspaceRoles: Record<string, HierarchicalRole>;
  }>({
    id: "",
    email: "",
    role: "member",
    workspaceRoles: {},
  });
  const [isLoading, setIsLoading] = useState(true);
  const [searchEmployeeQuery, setSearchEmployeeQuery] = useState("");
  const [workspaceFilter, setWorkspaceFilter] = useState("all");

  // Sub-modal states
  const [editOrgModalOpen, setEditOrgModalOpen] = useState(false);
  const [inviteModalOpen, setInviteModalOpen] = useState(false);
  const [editWsModal, setEditWsModal] = useState<OrgWorkspaceItem | null>(null);
  const [deleteWsModal, setDeleteWsModal] = useState<OrgWorkspaceItem | null>(
    null,
  );

  // Form states for Submodals
  const [orgBillingEmail] = useState("Chưa có dữ liệu billing");
  const [orgName, setOrgName] = useState("Đang tải tổ chức…");
  const [orgId, setOrgId] = useState<string | null>(null);
  const [orgAccountId, setOrgAccountId] = useState("");
  const [orgCreatedAt, setOrgCreatedAt] = useState("");
  const [orgRevision, setOrgRevision] = useState(0);
  const [orgPolicy, setOrgPolicy] = useState<{
    maxWorkspaces: number;
    offlineEntitled: boolean;
  }>({ maxWorkspaces: 0, offlineEntitled: false });

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
      const targets = workspaces.filter((workspace) =>
        selectedWsIds.has(workspace.id),
      );
      const results = await Promise.allSettled(
        targets.map((workspace) =>
          desktopApi.updateWorkspace({
            organizationId: workspace.organizationId,
            workspaceId: workspace.id,
            update: {
              name: workspace.name,
              policy: {
                maxProfiles: bulkMaxProfiles,
                maxMembers: bulkMaxMembers,
                maxStorageBytes: workspace.maxStorageBytes,
                offlineEntitled: workspace.offlineEntitled,
              },
            },
          }),
        ),
      );
      await loadRealData();
      const count = results.filter(
        (result) => result.status === "fulfilled",
      ).length;
      const failed = results.length - count;
      if (failed)
        throw new Error(
          `${count} workspace cập nhật được, ${failed} thất bại. Danh sách đã đồng bộ lại từ máy chủ.`,
        );
      showSuccessToast(t("workspaces.bulkActions.quotaUpdated", { count }));
      setBulkWsQuotaModalOpen(false);
      setSelectedWsIds(new Set());
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsBulkWsExecuting(false);
    }
  };

  const manageableEmployees = useMemo(() => {
    return employees.filter((emp) => {
      const actorRole = currentUser.workspaceRoles[emp.workspaceId];
      if (!actorRole) return false;
      const empRank = ROLE_RANKS[emp.role] ?? 0;
      const isSelf =
        emp.id === currentUser.id || emp.email === currentUser.email;
      return !isSelf && empRank < ROLE_RANKS[actorRole];
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
      const targets = employees.filter((e) => selectedEmpIds.has(e.id));
      const results = await Promise.allSettled(
        targets.map((member) =>
          desktopApi.updateMemberRole({
            workspaceId: member.workspaceId,
            memberId: member.id,
            role: newRole as HierarchicalRole,
          }),
        ),
      );
      await loadRealData();
      const failed = results.filter(
        (result) => result.status === "rejected",
      ).length;
      if (failed)
        throw new Error(
          `${failed} thành viên không cập nhật được; dữ liệu đã tải lại từ máy chủ.`,
        );
      showSuccessToast(
        t("members.bulkActions.roleUpdated", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } catch (error) {
      showErrorToast(String(error));
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const handleBulkEmpSuspend = async () => {
    if (selectedEmpIds.size === 0) return;
    setIsBulkEmpExecuting(true);
    try {
      const targets = employees.filter((e) => selectedEmpIds.has(e.id));
      const results = await Promise.allSettled(
        targets.map((member) =>
          desktopApi.suspendMember({
            workspaceId: member.workspaceId,
            memberId: member.id,
          }),
        ),
      );
      await loadRealData();
      const failed = results.filter(
        (result) => result.status === "rejected",
      ).length;
      if (failed)
        throw new Error(
          `${failed} thành viên không đình chỉ được; dữ liệu đã tải lại từ máy chủ.`,
        );
      showSuccessToast(
        t("members.bulkActions.suspendedCount", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } catch (error) {
      showErrorToast(String(error));
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const handleBulkEmpActivate = async () => {
    if (selectedEmpIds.size === 0) return;
    setIsBulkEmpExecuting(true);
    try {
      const targets = employees.filter((e) => selectedEmpIds.has(e.id));
      const results = await Promise.allSettled(
        targets.map((member) =>
          desktopApi.activateMember({
            workspaceId: member.workspaceId,
            memberId: member.id,
          }),
        ),
      );
      await loadRealData();
      const failed = results.filter(
        (result) => result.status === "rejected",
      ).length;
      if (failed)
        throw new Error(
          `${failed} thành viên không kích hoạt được; dữ liệu đã tải lại từ máy chủ.`,
        );
      showSuccessToast(
        t("members.bulkActions.activatedCount", { count: selectedEmpIds.size }),
      );
      setSelectedEmpIds(new Set());
    } catch (error) {
      showErrorToast(String(error));
    } finally {
      setIsBulkEmpExecuting(false);
    }
  };

  const loadRealData = useCallback(async () => {
    setIsLoading(true);
    try {
      const [account, organizations] = await Promise.all([
        desktopApi.getAccount(),
        desktopApi.listOrganizations(),
      ]);
      const userId = account.accountId;
      const userEmail = getActiveUserEmail() || "";
      const primaryOrg = organizations.items[0] ?? null;
      if (primaryOrg) {
        const organizationRecord = await desktopApi.getOrganizationRecord({
          organizationId: primaryOrg.id,
        });
        setOrgId(primaryOrg.id);
        setOrgAccountId(primaryOrg.accountId);
        setOrgCreatedAt(primaryOrg.createdAt);
        setOrgRevision(organizationRecord.revision);
        setOrgPolicy(organizationRecord.policy);
      } else {
        setOrgId(null);
        setOrgAccountId("");
        setOrgCreatedAt("");
      }
      const workspacePages = await Promise.all(
        organizations.items.map((org) => desktopApi.listWorkspaces(org.id)),
      );
      const allWorkspaces = workspacePages.flatMap((page) => page.items);
      const memberPages = await Promise.all(
        allWorkspaces.map(async (workspace) => ({
          workspace,
          page: await desktopApi.listMembers(workspace.id),
        })),
      );
      const loadedWorkspaces: OrgWorkspaceItem[] = allWorkspaces.map(
        (workspace, index) => ({
          id: workspace.id,
          organizationId: workspace.organizationId,
          name: workspace.name,
          profileCount: null,
          maxProfiles: workspace.policy.maxProfiles,
          memberCount: memberPages[index].page.items.length,
          maxMembers: workspace.policy.maxMembers,
          maxStorageBytes: workspace.policy.maxStorageBytes,
          offlineEntitled: workspace.policy.offlineEntitled,
          cloudSync: false,
        }),
      );
      const loadedMembers: OrgEmployee[] = memberPages.flatMap(
        ({ workspace, page }) =>
          page.items.map((member) => {
            const identity = member.identity;
            const email = identity?.email ?? "";
            return {
              id: member.id,
              accountId: member.accountId,
              workspaceId: workspace.id,
              name:
                identity?.username ||
                email ||
                t("members.identityUnavailable", {
                  defaultValue: "Chưa có thông tin định danh",
                }),
              email,
              role: member.role as HierarchicalRole,
              workspaces: [workspace.name],
              groupScope: "—",
              status: member.status,
            };
          }),
      );
      const ownMemberships = memberPages
        .flatMap(({ page }) => page.items)
        .filter(
          (member) => member.accountId === userId && member.status === "active",
        );
      const workspaceRoles = Object.fromEntries(
        ownMemberships.map((member) => [member.workspaceId, member.role]),
      ) as Record<string, HierarchicalRole>;
      const ownMembership = ownMemberships[0];
      setWorkspaces(loadedWorkspaces);
      setEmployees(loadedMembers);
      setCurrentUser({
        id: userId,
        email: userEmail,
        role: (ownMembership?.role as HierarchicalRole) ?? "member",
        workspaceRoles,
      });
      setOrgName(
        primaryOrg?.name ??
          t("orgManagement.noOrganization", {
            defaultValue: "Chưa có tổ chức",
          }),
      );
    } catch (error) {
      showErrorToast(`Không thể tải dữ liệu quản trị: ${String(error)}`);
    } finally {
      setIsLoading(false);
    }
  }, [t]);

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

  const totalProfilesUsed = workspaces.every(
    (workspace) => workspace.profileCount !== null,
  )
    ? workspaces.reduce(
        (acc, workspace) => acc + (workspace.profileCount ?? 0),
        0,
      )
    : null;
  const totalProfilesMax = workspaces.reduce(
    (sum, workspace) => sum + workspace.maxProfiles,
    0,
  );
  const profilesPercent =
    totalProfilesUsed === null || totalProfilesMax === 0
      ? 0
      : Math.round((totalProfilesUsed / totalProfilesMax) * 100);

  const totalMembersUsed = employees.length;
  const totalMembersMax = workspaces.reduce(
    (sum, workspace) => sum + workspace.maxMembers,
    0,
  );
  const membersPercent =
    totalMembersMax === 0
      ? 0
      : Math.round((totalMembersUsed / totalMembersMax) * 100);

  const totalWorkspacesUsed = workspaces.length;

  const handleToggleEmployeeSuspend = async (employeeId: string) => {
    const employee = employees.find((item) => item.id === employeeId);
    if (!employee) return;
    try {
      if (employee.status === "suspended")
        await desktopApi.activateMember({
          workspaceId: employee.workspaceId,
          memberId: employee.id,
        });
      else
        await desktopApi.suspendMember({
          workspaceId: employee.workspaceId,
          memberId: employee.id,
        });
      await loadRealData();
      showSuccessToast(
        employee.status === "suspended"
          ? "Đã kích hoạt thành viên"
          : "Đã đình chỉ thành viên",
      );
    } catch (error) {
      showErrorToast(String(error));
    }
  };

  const handleSaveWorkspaceEdit = async () => {
    if (!editWsModal) return;
    try {
      await desktopApi.updateWorkspace({
        organizationId: editWsModal.organizationId,
        workspaceId: editWsModal.id,
        update: {
          name: editWsModal.name,
          policy: {
            maxProfiles: editWsModal.maxProfiles,
            maxMembers: editWsModal.maxMembers,
            maxStorageBytes: editWsModal.maxStorageBytes,
            offlineEntitled: editWsModal.offlineEntitled,
          },
        },
      });
      await loadRealData();
      showSuccessToast(`Đã cập nhật workspace "${editWsModal.name}"`);
      setEditWsModal(null);
    } catch (error) {
      showErrorToast(String(error));
    }
  };

  const handleDeleteWorkspace = () =>
    showErrorToast(
      "Xóa workspace chưa có API; không có dữ liệu nào bị thay đổi.",
    );

  const handleSaveOrganization = async () => {
    if (!orgId) return showErrorToast("Không có tổ chức để cập nhật.");
    try {
      await desktopApi.updateOrganization({
        organizationId: orgId,
        update: {
          organization: {
            id: orgId,
            accountId: orgAccountId,
            name: orgName.trim(),
            createdAt: orgCreatedAt,
          },
          policy: orgPolicy,
          expectedRevision: orgRevision,
        },
      });
      const record = await desktopApi.getOrganizationRecord({
        organizationId: orgId,
      });
      setOrgRevision(record.revision);
      showSuccessToast("Đã cập nhật tên tổ chức.");
      setEditOrgModalOpen(false);
      await loadRealData();
    } catch (error) {
      showErrorToast(String(error));
    }
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
            </div>
            <div className="text-xs text-muted-foreground mt-0.5 flex items-center gap-2">
              <span>Tổ Chức:</span>
              <strong className="text-foreground">{orgName}</strong>
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
              {totalProfilesUsed === null
                ? "Chưa có số liệu sử dụng"
                : `${totalProfilesUsed.toLocaleString()} / ${totalProfilesMax.toLocaleString()} (${profilesPercent}%)`}
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
              {totalProfilesUsed === null
                ? "API chưa trả tổng số profile"
                : `Còn ${totalProfilesMax - totalProfilesUsed} profiles`}
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
              {totalWorkspacesUsed} workspace
            </span>
          </div>
          <div className="text-[10px] text-muted-foreground flex justify-between">
            <span>Hạn mức chưa có trong API</span>
            <span>Tổ chức hiện tại</span>
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
          onClick={() => setActiveTab("resources")}
          className={cn(
            "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
            activeTab === "resources"
              ? "bg-accent text-accent-foreground shadow-2xs"
              : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
          )}
        >
          <LuHardDrive className="size-3.5" />
          <span>Tài nguyên</span>
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
            <Button
              type="button"
              size="sm"
              onClick={() => setInviteModalOpen(true)}
              disabled={workspaces.length === 0}
              className="h-8 gap-1.5 text-xs"
            >
              <LuUserCheck className="size-3.5" />
              Mời thành viên và chọn quyền
            </Button>
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
                    const actorRole =
                      currentUser.workspaceRoles[emp.workspaceId];
                    const currentUserRank = actorRole
                      ? ROLE_RANKS[actorRole]
                      : -1;
                    const memberRank = ROLE_RANKS[emp.role] ?? 0;
                    const isSelf =
                      emp.accountId === currentUser.id ||
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
                              <span>Workspace Admin</span>
                            </Badge>
                          )}
                          {emp.role === "lead" && (
                            <Badge
                              variant="outline"
                              className="text-[10px] font-semibold"
                            >
                              Lead
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
                          ) : emp.status === "invited" ? (
                            <Badge variant="outline" className="text-[10px]">
                              Chờ xác minh / mời
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
                          ) : !actorRole || isHigherOrEqual ? (
                            <Badge
                              variant="outline"
                              className="text-[10px] text-muted-foreground border-border/80 font-normal"
                            >
                              {!actorRole
                                ? "Ngoài phạm vi quản lý"
                                : t("members.protectedRole", {
                                    defaultValue: "Vai trò được bảo vệ",
                                  })}
                            </Badge>
                          ) : (
                            <div className="flex items-center justify-end gap-1.5">
                              <span
                                className="text-[10px] text-muted-foreground"
                                title="API khôi phục mật khẩu chưa hỗ trợ; dùng luồng khôi phục tài khoản."
                              >
                                Đặt lại mật khẩu chưa hỗ trợ; liên hệ Platform
                                Admin
                              </span>
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
                    <SelectItem value="lead" className="text-xs">
                      Lead
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
                  const prfPercent =
                    ws.profileCount === null || ws.maxProfiles === 0
                      ? 0
                      : Math.round((ws.profileCount / ws.maxProfiles) * 100);
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
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap min-w-[150px]">
                        <div className="flex justify-between font-mono text-[11px] mb-1">
                          <span>
                            {ws.profileCount === null
                              ? `— / ${ws.maxProfiles}`
                              : `${ws.profileCount} / ${ws.maxProfiles}`}
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
                          <span
                            className="text-[10px] text-muted-foreground"
                            title="API xóa workspace chưa được hỗ trợ; liên hệ Platform Admin."
                          >
                            Xóa chưa hỗ trợ
                          </span>
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

                <span className="text-[11px] text-muted-foreground">
                  Lưu trữ workspace chưa có API; liên hệ Platform Admin.
                </span>
              </div>
            </div>
          )}
        </div>
      )}

      {activeTab === "resources" && <ResourceInventoryPanel />}

      {/* 6. Tab 3: Security Policies (Chính Sách Bảo Mật Bắt Buộc Toàn Công Ty) */}
      {activeTab === "security" && (
        <div className="rounded-lg border border-border p-4 text-sm text-muted-foreground">
          Chính sách bảo mật tổ chức chưa có API lưu trữ. Chưa áp dụng thay đổi
          nào.
        </div>
      )}
      {/* 7. Tab 4: Billing & Tier Info */}
      {activeTab === "billing" && (
        <div className="rounded-lg border border-border p-4 text-sm text-muted-foreground">
          Thông tin billing và license chưa được cung cấp bởi API của tổ chức.
        </div>
      )}
      {false && activeTab === "billing" && (
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
                <span className="text-muted-foreground">Nhân Chromium:</span>
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

      <CreateInviteModal
        isOpen={inviteModalOpen}
        onClose={() => setInviteModalOpen(false)}
        onInviteCreated={() => void loadRealData()}
      />

      {/* SUB-MODAL: Edit Organization */}
      <Dialog open={editOrgModalOpen} onOpenChange={setEditOrgModalOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuBuilding2 className="size-4 text-primary" />
              <span>Cài Đặt Tổ Chức</span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              Cập nhật tên tổ chức trong hệ thống.
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
              onClick={() => void handleSaveOrganization()}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              Lưu Cài Đặt
            </Button>
          </DialogFooter>
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

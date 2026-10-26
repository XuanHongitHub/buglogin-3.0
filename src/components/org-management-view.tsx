"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowRight,
  LuBan,
  LuBuilding2,
  LuCrown,
  LuHardDrive,
  LuPencil,
  LuSearch,
  LuShield,
  LuShieldCheck,
  LuSlidersHorizontal,
  LuUserCheck,
  LuUsers,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
import { canOpenOrganizationMembers } from "@/components/org-member-access";
import {
  type OrgWorkspaceRoute,
  parseOrgWorkspaceRoute,
  sameOrgWorkspaceRoute,
  serializeOrgWorkspaceRoute,
} from "@/components/org-workspace-route";
import { OrganizationMembersPage } from "@/components/organization-members-page";
import { OrganizationWorkspaceAccess } from "@/components/organization-workspace-access";
import { ResourceInventoryPanel } from "@/components/resource-inventory-panel";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ManagementDrawerContent } from "@/components/ui/management-drawer";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { WorkspaceDetailPage } from "@/components/workspace-detail-page";
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
  status: "invited" | "active" | "suspended";
}

export interface OrgWorkspaceItem {
  id: string;
  organizationId: string;
  name: string;
  maxProfiles: number;
  memberCount: number;
  maxMembers: number;
  maxStorageBytes: number;
  offlineEntitled: boolean;
}

interface OrgManagementCenterProps {
  mode?: "page" | "dialog";
  isOpen?: boolean;
  onClose?: () => void;
}

export function OrgManagementCenter({
  mode = "dialog",
  isOpen = true,
  onClose,
}: OrgManagementCenterProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<
    "members" | "access" | "workspaces" | "resources"
  >("workspaces");

  const [employees, setEmployees] = useState<OrgEmployee[]>([]);
  const [workspaces, setWorkspaces] = useState<OrgWorkspaceItem[]>([]);
  const [organizations, setOrganizations] = useState<
    Awaited<ReturnType<typeof desktopApi.listOrganizations>>["items"]
  >([]);
  const [currentUser, setCurrentUser] = useState<{
    id: string;
    email: string;
    platformAdmin: boolean;
    role: HierarchicalRole;
    workspaceRoles: Record<string, HierarchicalRole>;
  }>({
    id: "",
    email: "",
    platformAdmin: false,
    role: "member",
    workspaceRoles: {},
  });
  const [isLoading, setIsLoading] = useState(true);
  const [searchEmployeeQuery, setSearchEmployeeQuery] = useState("");
  const [workspaceFilter, setWorkspaceFilter] = useState("all");

  // Sub-modal states
  const [editOrgModalOpen, setEditOrgModalOpen] = useState(false);
  const [accountCreationPageOpen, setAccountCreationPageOpen] = useState(false);
  const [organizationMembersPageOpen, setOrganizationMembersPageOpen] =
    useState(false);
  const [accountWorkspaceId, setAccountWorkspaceId] = useState("");
  const [workspaceDetailPage, setWorkspaceDetailPage] =
    useState<OrgWorkspaceItem | null>(null);
  const [workspaceDetailInviteOpen, setWorkspaceDetailInviteOpen] =
    useState(false);
  const [route, setRoute] = useState<OrgWorkspaceRoute>(() =>
    typeof window === "undefined"
      ? {}
      : parseOrgWorkspaceRoute(window.location.search),
  );
  const routeRef = useRef(route);
  const [routeIssue, setRouteIssue] = useState<string | null>(null);
  const [editWsModal, setEditWsModal] = useState<OrgWorkspaceItem | null>(null);

  // Form states for Submodals
  const [orgName, setOrgName] = useState("Đang tải tổ chức…");
  const [orgId, setOrgId] = useState<string | null>(null);
  const [orgAccountId, setOrgAccountId] = useState("");
  const [orgCreatedAt, setOrgCreatedAt] = useState("");
  const [orgRevision, setOrgRevision] = useState(0);
  const [orgPolicy, setOrgPolicy] = useState<{
    maxWorkspaces: number;
    offlineEntitled: boolean;
  }>({ maxWorkspaces: 0, offlineEntitled: false });

  const navigateRoute = useCallback(
    (next: OrgWorkspaceRoute, replace = false) => {
      if (typeof window === "undefined") return;
      const normalized = {
        ...next,
        section: next.workspaceId ? (next.section ?? "overview") : undefined,
        groupId: next.section === "groups" ? next.groupId : undefined,
        profileId: next.section === "profiles" ? next.profileId : undefined,
      };
      if (sameOrgWorkspaceRoute(routeRef.current, normalized)) return;
      const url = serializeOrgWorkspaceRoute(
        window.location.search,
        normalized,
      );
      if (replace) window.history.replaceState(null, "", url);
      else window.history.pushState(null, "", url);
      routeRef.current = normalized;
      setRoute(normalized);
      setRouteIssue(null);
    },
    [],
  );

  useEffect(() => {
    const onPopState = () => {
      const next = parseOrgWorkspaceRoute(window.location.search);
      routeRef.current = next;
      setRoute(next);
      setRouteIssue(null);
    };
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

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
    const requestedRoute = routeRef.current;
    setIsLoading(true);
    try {
      const [account, organizations] = await Promise.all([
        desktopApi.getAccount(),
        desktopApi.listOrganizations(),
      ]);
      setOrganizations(organizations.items);
      const userId = account.accountId;
      const userEmail = getActiveUserEmail() || "";
      const requestedOrg = organizations.items.find(
        (org) => org.id === requestedRoute.organizationId,
      );
      const primaryOrg = requestedOrg ?? organizations.items[0] ?? null;
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
      const workspacePages = primaryOrg
        ? [await desktopApi.listWorkspaces(primaryOrg.id)]
        : [];
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
          maxProfiles: workspace.policy.maxProfiles,
          memberCount: memberPages[index].page.items.length,
          maxMembers: workspace.policy.maxMembers,
          maxStorageBytes: workspace.policy.maxStorageBytes,
          offlineEntitled: workspace.policy.offlineEntitled,
        }),
      );
      if (requestedRoute.workspaceId) {
        const requestedWorkspace = loadedWorkspaces.find(
          (workspace) =>
            workspace.id === requestedRoute.workspaceId &&
            (!requestedRoute.organizationId ||
              workspace.organizationId === requestedRoute.organizationId),
        );
        if (requestedWorkspace) {
          setWorkspaceDetailPage(requestedWorkspace);
          setWorkspaceDetailInviteOpen(false);
          setRouteIssue(null);
          navigateRoute(
            {
              organizationId: requestedWorkspace.organizationId,
              workspaceId: requestedWorkspace.id,
              section: requestedRoute.section ?? "overview",
              groupId: requestedRoute.groupId,
              profileId: requestedRoute.profileId,
            },
            true,
          );
        } else {
          setWorkspaceDetailPage(null);
          setRouteIssue(
            "Workspace này không còn khả dụng hoặc tài khoản hiện tại không có quyền truy cập. Đang quay lại danh sách tổ chức.",
          );
          navigateRoute({ organizationId: primaryOrg?.id }, true);
        }
      } else if (requestedRoute.organizationId && !requestedOrg) {
        setRouteIssue(
          "Tổ chức này không còn khả dụng hoặc tài khoản hiện tại không có quyền truy cập.",
        );
        navigateRoute({ organizationId: primaryOrg?.id }, true);
      }
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
              status: member.status,
            };
          }),
      );
      const ownMemberships = memberPages
        .flatMap(({ page }) => page.items)
        .filter(
          (member) => member.accountId === userId && member.status === "active",
        );
      const visibleWorkspaceIds = new Set(allWorkspaces.map(({ id }) => id));
      const workspaceRoles = Object.fromEntries(
        account.managedWorkspaceIds
          .filter((managedWorkspaceId) =>
            visibleWorkspaceIds.has(managedWorkspaceId),
          )
          .map((managedWorkspaceId) => [managedWorkspaceId, "owner"]),
      ) as Record<string, HierarchicalRole>;
      Object.assign(
        workspaceRoles,
        Object.fromEntries(
          ownMemberships
            .filter(
              (member) =>
                visibleWorkspaceIds.has(member.workspaceId) &&
                !account.managedWorkspaceIds.includes(member.workspaceId),
            )
            .map((member) => [member.workspaceId, member.role]),
        ),
      );
      const ownMembership = ownMemberships[0];
      setWorkspaces(loadedWorkspaces);
      setEmployees(loadedMembers);
      setCurrentUser({
        id: userId,
        email: userEmail,
        platformAdmin: account.platformAdmin,
        role: (ownMembership?.role as HierarchicalRole) ?? "member",
        workspaceRoles,
      });
      const manageableWorkspaceIds = Object.keys(workspaceRoles);
      setAccountWorkspaceId((current) =>
        manageableWorkspaceIds.includes(current)
          ? current
          : (manageableWorkspaceIds[0] ?? ""),
      );
      setOrgName(
        primaryOrg?.name ??
          t("orgManagement.noOrganization", {
            defaultValue: "Chưa có tổ chức",
          }),
      );
    } catch (error) {
      showErrorToast(`Không thể tải dữ liệu quản trị: ${String(error)}`);
      if (requestedRoute.workspaceId) {
        setWorkspaceDetailPage(null);
        setRouteIssue(
          "Không thể xác nhận quyền truy cập workspace từ liên kết này. Đang quay lại phạm vi tổ chức.",
        );
        navigateRoute({ organizationId: requestedRoute.organizationId }, true);
      } else if (requestedRoute.organizationId) {
        setRouteIssue(
          "Không thể xác nhận quyền truy cập tổ chức từ liên kết này.",
        );
      }
    } finally {
      setIsLoading(false);
    }
  }, [navigateRoute, t]);

  useEffect(() => {
    if (isOpen) {
      void loadRealData();
    }
  }, [isOpen, loadRealData]);

  useEffect(() => {
    if (!isOpen || isLoading) return;
    if ((route.organizationId ?? null) !== orgId) void loadRealData();
  }, [isOpen, isLoading, loadRealData, orgId, route.organizationId]);

  useEffect(() => {
    if (isLoading) return;
    if (!route.workspaceId) {
      setWorkspaceDetailPage(null);
      setWorkspaceDetailInviteOpen(false);
      return;
    }
    const workspace = workspaces.find(
      (item) =>
        item.id === route.workspaceId &&
        (!route.organizationId || item.organizationId === route.organizationId),
    );
    if (workspace) {
      setWorkspaceDetailPage(workspace);
      if (workspaceDetailPage?.id !== workspace.id)
        setWorkspaceDetailInviteOpen(false);
      return;
    }
    setWorkspaceDetailPage(null);
    setRouteIssue(
      "Workspace này không còn khả dụng hoặc tài khoản hiện tại không có quyền truy cập.",
    );
    navigateRoute({ organizationId: route.organizationId }, true);
  }, [isLoading, navigateRoute, route, workspaceDetailPage?.id, workspaces]);

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

  const canManageOrganizationAccounts = canOpenOrganizationMembers({
    platformAdmin: currentUser.platformAdmin,
    organizationOwner: !!orgAccountId && currentUser.id === orgAccountId,
    workspaceRoles: Object.values(currentUser.workspaceRoles),
  });

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
    <div className="space-y-3 text-xs font-sans">
      {/* 1. Header Bar */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/50 pb-3">
        <div className="flex items-center gap-3">
          <div className="grid size-9 place-items-center rounded-md bg-muted text-foreground">
            <LuBuilding2 className="size-4" />
          </div>
          <div className="flex min-w-0 items-center gap-2">
            {routeIssue ? (
              <p role="status" className="truncate text-xs text-amber-600">
                {routeIssue}
              </p>
            ) : null}
            <div className="text-xs text-muted-foreground mt-0.5 flex items-center gap-2">
              <span>Tổ chức:</span>
              {organizations.length > 0 ? (
                <Select
                  value={orgId ?? undefined}
                  onValueChange={(organizationId) =>
                    navigateRoute({ organizationId })
                  }
                >
                  <SelectTrigger
                    aria-label="Chọn tổ chức"
                    className="h-7 w-64 max-w-[45vw] border-0 bg-transparent px-1 text-xs font-semibold text-foreground shadow-none"
                  >
                    <SelectValue placeholder="Chọn tổ chức" />
                  </SelectTrigger>
                  <SelectContent>
                    {organizations.map((organization) => (
                      <SelectItem key={organization.id} value={organization.id}>
                        {organization.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <strong className="text-foreground">{orgName}</strong>
              )}
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
        </div>
      </div>

      {/* Organization scope tabs */}
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

        {canManageOrganizationAccounts ? (
          <button
            type="button"
            onClick={() => setActiveTab("access")}
            className={cn(
              "px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors duration-150 cursor-pointer flex items-center gap-1.5",
              activeTab === "access"
                ? "bg-accent text-accent-foreground shadow-2xs"
                : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
            )}
          >
            <LuShield className="size-3.5" />
            <span>Workspace Access</span>
          </button>
        ) : null}

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
            <div className="flex items-center gap-2">
              {canManageOrganizationAccounts && orgId && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setOrganizationMembersPageOpen(true)}
                  className="h-8 gap-1.5 text-xs"
                >
                  <LuUsers className="size-3.5" />
                  Tài khoản tổ chức
                </Button>
              )}
              {Object.keys(currentUser.workspaceRoles).length > 0 && (
                <Select
                  value={accountWorkspaceId}
                  onValueChange={setAccountWorkspaceId}
                >
                  <SelectTrigger className="h-8 w-[190px] text-xs">
                    <SelectValue placeholder="Chọn workspace" />
                  </SelectTrigger>
                  <SelectContent>
                    {workspaces
                      .filter(
                        (workspace) => currentUser.workspaceRoles[workspace.id],
                      )
                      .map((workspace) => (
                        <SelectItem
                          key={workspace.id}
                          value={workspace.id}
                          className="text-xs"
                        >
                          {workspace.name}
                        </SelectItem>
                      ))}
                  </SelectContent>
                </Select>
              )}
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => setAccountCreationPageOpen(true)}
                disabled={!accountWorkspaceId}
                className="h-8 gap-1.5 text-xs"
              >
                <LuUsers className="size-3.5" />
                Tạo tài khoản
              </Button>
              <Button
                type="button"
                size="sm"
                onClick={() => {
                  const workspace = workspaces.find(
                    (item) => item.id === accountWorkspaceId,
                  );
                  if (workspace) {
                    setWorkspaceDetailInviteOpen(true);
                    setWorkspaceDetailPage(workspace);
                    navigateRoute({
                      organizationId: workspace.organizationId,
                      workspaceId: workspace.id,
                      section: "members",
                    });
                  }
                }}
                disabled={!accountWorkspaceId}
                className="h-8 gap-1.5 text-xs"
              >
                <LuUserCheck className="size-3.5" />
                Mời thành viên và chọn quyền
              </Button>
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
                  <th className="py-2.5 px-3">Vai trò tại workspace</th>
                  <th className="py-2.5 px-3">Workspace</th>
                  <th className="py-2.5 px-3">Trạng Thái</th>
                  <th className="py-2.5 px-3 text-right">Thao Tác Quản Trị</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {isLoading ? (
                  <tr>
                    <td
                      colSpan={6}
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
                      colSpan={6}
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
                          {emp.workspaces[0] ?? "Workspace không xác định"}
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
                {workspaces.map((ws) => (
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
                    <td className="py-2.5 px-3 whitespace-nowrap font-mono text-[11px] text-muted-foreground">
                      {ws.maxProfiles} max
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
                      </div>
                    </td>
                    <td className="py-2.5 px-3 text-right whitespace-nowrap">
                      <div className="flex items-center justify-end gap-1.5 whitespace-nowrap">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => {
                            setWorkspaceDetailInviteOpen(false);
                            setWorkspaceDetailPage(ws);
                            navigateRoute({
                              organizationId: ws.organizationId,
                              workspaceId: ws.id,
                              section: "overview",
                            });
                          }}
                          className="h-7 px-2.5 text-xs gap-1 cursor-pointer font-medium"
                        >
                          <LuArrowRight className="size-3" />
                          <span>Mở workspace</span>
                        </Button>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              type="button"
                              variant="outline"
                              size="icon"
                              aria-label={`Thao tác với ${ws.name}`}
                              className="size-7"
                            >
                              <LuSlidersHorizontal className="size-3.5" />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end" className="w-48">
                            <DropdownMenuItem
                              onSelect={() => {
                                setWorkspaceDetailInviteOpen(false);
                                setWorkspaceDetailPage(ws);
                                navigateRoute({
                                  organizationId: ws.organizationId,
                                  workspaceId: ws.id,
                                  section: "members",
                                });
                              }}
                            >
                              Thành viên và quyền
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              onSelect={() => setEditWsModal(ws)}
                            >
                              <LuPencil />
                              Sửa workspace
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </div>
                    </td>
                  </tr>
                ))}
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

      {activeTab === "access" && canManageOrganizationAccounts && orgId ? (
        <OrganizationWorkspaceAccess
          organizationId={orgId}
          workspaces={workspaces.map(({ id, name }) => ({ id, name }))}
          allowOwner={currentUser.platformAdmin}
          listOrganizationMembers={desktopApi.listOrganizationMembers}
          listWorkspaceMembers={desktopApi.listMembers}
          assignMember={desktopApi.assignOrganizationMemberToWorkspace}
        />
      ) : null}

      {activeTab === "resources" && <ResourceInventoryPanel />}

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
        <ManagementDrawerContent>
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
        </ManagementDrawerContent>
      </Dialog>

      {/* SUB-MODAL: Edit Workspace Settings (Mục 14.2) */}
      {editWsModal && (
        <Dialog open={true} onOpenChange={() => setEditWsModal(null)}>
          <ManagementDrawerContent>
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
          </ManagementDrawerContent>
        </Dialog>
      )}
    </div>
  );

  if (organizationMembersPageOpen) {
    if (!canManageOrganizationAccounts || !orgId) {
      return (
        <main className="flex min-h-0 flex-1 flex-col bg-background">
          <div className="border-b border-border/60 px-4 py-3 md:px-6">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setOrganizationMembersPageOpen(false)}
            >
              Quay lại tổ chức
            </Button>
          </div>
          <p role="alert" className="p-6 text-sm text-destructive">
            Bạn không có quyền quản lý tài khoản tổ chức này.
          </p>
        </main>
      );
    }
    const inviteWorkspace =
      workspaces.find((workspace) => workspace.id === accountWorkspaceId) ??
      workspaces.find((workspace) => currentUser.workspaceRoles[workspace.id]);
    return (
      <OrganizationMembersPage
        organizationId={orgId}
        organizationName={orgName}
        canInviteMember={Boolean(inviteWorkspace)}
        onBack={() => setOrganizationMembersPageOpen(false)}
        onInviteMember={() => {
          if (!inviteWorkspace) return;
          setOrganizationMembersPageOpen(false);
          setWorkspaceDetailInviteOpen(true);
          setWorkspaceDetailPage(inviteWorkspace);
          navigateRoute({
            organizationId: inviteWorkspace.organizationId,
            workspaceId: inviteWorkspace.id,
            section: "members",
          });
        }}
        listMembers={desktopApi.listOrganizationMembers}
        createAccount={desktopApi.createOrganizationAccount}
        updateMemberStatus={desktopApi.updateOrganizationMemberStatus}
      />
    );
  }

  if (workspaceDetailPage) {
    return (
      <WorkspaceDetailPage
        organizationId={workspaceDetailPage.organizationId}
        organizationName={orgName}
        workspaceId={workspaceDetailPage.id}
        workspaceName={workspaceDetailPage.name}
        initialTab={
          route.workspaceId === workspaceDetailPage.id
            ? route.section
            : "overview"
        }
        initialGroupId={route.groupId}
        initialProfileId={route.profileId}
        initialInvitePage={workspaceDetailInviteOpen}
        onRouteChange={(next) => navigateRoute(next)}
        onBack={() => {
          setWorkspaceDetailPage(null);
          setWorkspaceDetailInviteOpen(false);
          navigateRoute({ organizationId: workspaceDetailPage.organizationId });
        }}
      />
    );
  }

  if (accountCreationPageOpen && accountWorkspaceId) {
    return (
      <Dialog
        open
        onOpenChange={(open) => !open && setAccountCreationPageOpen(false)}
      >
        <ManagementDrawerContent
          width="lg"
          hideClose
          className="!overflow-hidden !p-0"
        >
          <ScopedAccountCreationPage
            workspaceId={accountWorkspaceId}
            workspaceName={
              workspaces.find(
                (workspace) => workspace.id === accountWorkspaceId,
              )?.name ?? "Workspace"
            }
            organizationName={orgName}
            onBack={() => setAccountCreationPageOpen(false)}
            onCreated={() => void loadRealData()}
          />
        </ManagementDrawerContent>
      </Dialog>
    );
  }

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

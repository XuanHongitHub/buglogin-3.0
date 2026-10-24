"use client";

import { useCallback, useEffect, useState } from "react";
import {
  LuArrowLeft,
  LuCheck,
  LuCopy,
  LuFolder,
  LuLayoutGrid,
  LuPlus,
  LuRefreshCw,
  LuUsers,
} from "react-icons/lu";
import { MemberManagementDialog } from "@/components/member-management-dialog";
import { ResourceInventoryPanel } from "@/components/resource-inventory-panel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type {
  GroupRecord,
  GroupShareGrant,
  Member,
  MemberRole,
} from "../../frontend/src/generated/command-registry";

const desktopApi = createTauriDesktopApi();
type WorkspaceTab = "overview" | "members" | "groups" | "profiles";
const MEMBER_ROLE_RANK: Record<MemberRole, number> = {
  owner: 4,
  admin: 3,
  lead: 2,
  operator: 1,
  member: 0,
};
const MEMBER_ROLES: MemberRole[] = [
  "owner",
  "admin",
  "lead",
  "operator",
  "member",
];

export function WorkspaceDetailPage({
  organizationId,
  organizationName,
  workspaceId,
  workspaceName,
  onBack,
  initialTab = "overview",
  initialInvitePage = false,
}: {
  organizationId: string;
  organizationName: string;
  workspaceId: string;
  workspaceName: string;
  onBack: () => void;
  initialTab?: WorkspaceTab;
  initialInvitePage?: boolean;
}) {
  const [tab, setTab] = useState<WorkspaceTab>(initialTab);
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const [groupsLoading, setGroupsLoading] = useState(false);
  const [groupsError, setGroupsError] = useState<string | null>(null);
  const [newGroupName, setNewGroupName] = useState("");
  const [editingGroup, setEditingGroup] = useState<GroupRecord | null>(null);
  const [editingName, setEditingName] = useState("");
  const [confirmDeleteGroupId, setConfirmDeleteGroupId] = useState<
    string | null
  >(null);
  const [groupMutationBusy, setGroupMutationBusy] = useState(false);
  const [invitePageOpen, setInvitePageOpen] = useState(initialInvitePage);
  const [accessGroup, setAccessGroup] = useState<GroupRecord | null>(null);
  const [groupShares, setGroupShares] = useState<GroupShareGrant[]>([]);
  const [workspaceMembers, setWorkspaceMembers] = useState<Member[]>([]);
  const [shareMemberId, setShareMemberId] = useState("");
  const [shareAction, setShareAction] =
    useState<GroupShareGrant["action"]>("read");
  const [accessError, setAccessError] = useState<string | null>(null);
  const [accessBusy, setAccessBusy] = useState(false);

  const loadGroups = useCallback(async () => {
    setGroupsLoading(true);
    setGroupsError(null);
    try {
      setGroups(await desktopApi.listGroups(workspaceId));
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setGroupsLoading(false);
    }
  }, [workspaceId]);

  useEffect(() => {
    void loadGroups();
  }, [loadGroups]);

  const createGroup = async (event: React.FormEvent) => {
    event.preventDefault();
    const name = newGroupName.trim();
    if (!name || groupMutationBusy) return;
    setGroupMutationBusy(true);
    setGroupsError(null);
    try {
      const created = await desktopApi.createGroup({
        workspaceId,
        groupId: `grp_${crypto.randomUUID().replaceAll("-", "")}`,
        name,
      });
      setGroups((current) =>
        [...current, created].sort((a, b) =>
          a.group.name.localeCompare(b.group.name),
        ),
      );
      setNewGroupName("");
      showSuccessToast("Đã tạo group.");
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const saveGroupName = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!editingGroup || !editingName.trim() || groupMutationBusy) return;
    setGroupMutationBusy(true);
    setGroupsError(null);
    try {
      const updated = await desktopApi.updateGroup({
        workspaceId,
        groupId: editingGroup.group.id,
        expectedRevision: editingGroup.revision,
        name: editingName.trim(),
      });
      setGroups((current) =>
        current.map((item) =>
          item.group.id === updated.group.id ? updated : item,
        ),
      );
      setEditingGroup(null);
      showSuccessToast("Đã cập nhật tên group.");
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const deleteGroup = async (group: GroupRecord) => {
    if (groupMutationBusy) return;
    setGroupMutationBusy(true);
    setGroupsError(null);
    try {
      await desktopApi.deleteGroup({
        workspaceId,
        groupId: group.group.id,
        expectedRevision: group.revision,
      });
      setGroups((current) =>
        current.filter((item) => item.group.id !== group.group.id),
      );
      setConfirmDeleteGroupId(null);
      showSuccessToast("Đã xóa group.");
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const loadGroupAccess = async (record: GroupRecord) => {
    setAccessGroup(record);
    setAccessError(null);
    setAccessBusy(true);
    try {
      const [shares, members] = await Promise.all([
        desktopApi.listGroupShares({ workspaceId, groupId: record.group.id }),
        desktopApi.listMembers(workspaceId),
      ]);
      setGroupShares(shares);
      const activeMembers = members.items.filter(
        (member) => member.status === "active" && member.identity,
      );
      setWorkspaceMembers(activeMembers);
      setShareMemberId(activeMembers[0]?.id ?? "");
    } catch (error) {
      setAccessError(error instanceof Error ? error.message : String(error));
    } finally {
      setAccessBusy(false);
    }
  };

  const addGroupShare = async () => {
    if (!accessGroup || !shareMemberId || accessBusy) return;
    setAccessBusy(true);
    setAccessError(null);
    try {
      const created = await desktopApi.createGroupShare({
        workspaceId,
        groupId: accessGroup.group.id,
        memberId: shareMemberId,
        action: shareAction,
        effect: "allow",
      });
      setGroupShares((current) => [...current, created]);
      showSuccessToast("Đã cấp quyền group.");
    } catch (error) {
      setAccessError(error instanceof Error ? error.message : String(error));
    } finally {
      setAccessBusy(false);
    }
  };

  const revokeGroupShare = async (share: GroupShareGrant) => {
    if (!accessGroup || accessBusy) return;
    setAccessBusy(true);
    setAccessError(null);
    try {
      const revoked = await desktopApi.revokeGroupShare({
        workspaceId,
        groupId: accessGroup.group.id,
        grantId: share.id,
        expectedRevision: share.revision,
      });
      setGroupShares((current) =>
        current.filter((item) => item.id !== revoked.id),
      );
    } catch (error) {
      setAccessError(error instanceof Error ? error.message : String(error));
    } finally {
      setAccessBusy(false);
    }
  };

  const tabs: Array<{
    id: WorkspaceTab;
    label: string;
    icon: typeof LuLayoutGrid;
  }> = [
    { id: "overview", label: "Tổng quan", icon: LuLayoutGrid },
    { id: "members", label: "Thành viên", icon: LuUsers },
    { id: "groups", label: "Groups", icon: LuFolder },
    { id: "profiles", label: "Profiles", icon: LuLayoutGrid },
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden bg-background">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-4 py-3 md:px-6">
        <div className="flex min-w-0 items-center gap-3">
          <Button
            variant="ghost"
            size="icon"
            onClick={onBack}
            aria-label="Quay lại danh sách workspace"
          >
            <LuArrowLeft className="size-4" />
          </Button>
          <div className="min-w-0">
            <p className="truncate text-xs text-muted-foreground">
              Organizations / {organizationName} /
            </p>
            <h1 className="truncate text-base font-semibold">
              {workspaceName}
            </h1>
          </div>
        </div>
        <nav aria-label="Workspace sections" className="flex flex-wrap gap-1">
          {tabs.map(({ id, label, icon: Icon }) => (
            <Button
              key={id}
              type="button"
              size="sm"
              variant={tab === id ? "secondary" : "ghost"}
              onClick={() => setTab(id)}
              aria-current={tab === id ? "page" : undefined}
            >
              <Icon className="mr-1.5 size-3.5" />
              {label}
            </Button>
          ))}
        </nav>
      </header>

      {tab === "members" ? (
        invitePageOpen ? (
          <WorkspaceInvitePage
            organizationName={organizationName}
            workspaceId={workspaceId}
            workspaceName={workspaceName}
            onBack={() => setInvitePageOpen(false)}
          />
        ) : (
          <MemberManagementDialog
            mode="page"
            isOpen
            onClose={() => setTab("overview")}
            onInvitePage={() => setInvitePageOpen(true)}
            workspaceId={workspaceId}
            workspaceName={workspaceName}
            organizationName={organizationName}
          />
        )
      ) : tab === "profiles" ? (
        <ResourceInventoryPanel
          initialOrganizationId={organizationId}
          initialWorkspaceId={workspaceId}
          initialWorkspaceName={workspaceName}
          initialKind="profile"
          showWorkspaceSelector={false}
        />
      ) : tab === "groups" ? (
        <section
          className="min-h-0 flex-1 overflow-y-auto p-4 md:p-6"
          aria-labelledby="workspace-groups-title"
        >
          <div className="mx-auto max-w-5xl space-y-4">
            <header className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2
                  id="workspace-groups-title"
                  className="text-base font-semibold"
                >
                  Groups trong {workspaceName}
                </h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  Profile có thể thuộc nhiều group hoặc để trống group. Việc gán
                  profile được quản lý ở trang Profiles.
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void loadGroups()}
                disabled={groupsLoading}
              >
                <LuRefreshCw
                  className={`mr-1.5 size-3.5 ${groupsLoading ? "animate-spin" : ""}`}
                />{" "}
                Làm mới
              </Button>
            </header>

            <form
              onSubmit={(event) => void createGroup(event)}
              className="flex flex-wrap items-end gap-2 rounded-md border border-border p-3"
            >
              <div className="min-w-52 flex-1 space-y-1.5">
                <Label htmlFor="new-workspace-group">Tạo group</Label>
                <Input
                  id="new-workspace-group"
                  value={newGroupName}
                  onChange={(event) => setNewGroupName(event.target.value)}
                  maxLength={80}
                  required
                  placeholder="Tên group"
                />
              </div>
              <Button
                type="submit"
                disabled={!newGroupName.trim() || groupMutationBusy}
              >
                <LuPlus className="mr-1.5 size-4" />
                Tạo group
              </Button>
            </form>

            {groupsError && (
              <p
                role="alert"
                className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
              >
                Không thể hoàn tất thao tác group: {groupsError}
              </p>
            )}
            <div className="overflow-hidden rounded-md border border-border">
              <table className="w-full text-left text-sm">
                <thead className="bg-muted/40 text-xs text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2.5">Tên group</th>
                    <th className="px-3 py-2.5">Cập nhật</th>
                    <th className="px-3 py-2.5 text-right">Thao tác</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border">
                  {groupsLoading ? (
                    <tr>
                      <td
                        colSpan={3}
                        className="px-3 py-8 text-center text-muted-foreground"
                      >
                        Đang tải groups…
                      </td>
                    </tr>
                  ) : groups.length === 0 ? (
                    <tr>
                      <td
                        colSpan={3}
                        className="px-3 py-8 text-center text-muted-foreground"
                      >
                        Workspace chưa có group.
                      </td>
                    </tr>
                  ) : (
                    groups.map((record) => (
                      <tr key={record.group.id}>
                        <td className="px-3 py-2.5">
                          {editingGroup?.group.id === record.group.id ? (
                            <form
                              onSubmit={(event) => void saveGroupName(event)}
                              className="flex max-w-lg gap-2"
                            >
                              <Input
                                autoFocus
                                value={editingName}
                                onChange={(event) =>
                                  setEditingName(event.target.value)
                                }
                                maxLength={80}
                                aria-label="Tên group mới"
                              />
                              <Button
                                size="sm"
                                type="submit"
                                disabled={
                                  !editingName.trim() || groupMutationBusy
                                }
                              >
                                Lưu
                              </Button>
                              <Button
                                size="sm"
                                type="button"
                                variant="ghost"
                                onClick={() => setEditingGroup(null)}
                              >
                                Hủy
                              </Button>
                            </form>
                          ) : (
                            <span className="font-medium">
                              {record.group.name}
                            </span>
                          )}
                        </td>
                        <td className="px-3 py-2.5 text-xs text-muted-foreground">
                          {new Date(record.group.updatedAt).toLocaleString()}
                        </td>
                        <td className="px-3 py-2.5">
                          <div className="flex justify-end gap-2">
                            {confirmDeleteGroupId === record.group.id ? (
                              <>
                                <span className="self-center text-xs text-muted-foreground">
                                  Xác nhận xóa?
                                </span>
                                <Button
                                  size="sm"
                                  variant="destructive"
                                  disabled={groupMutationBusy}
                                  onClick={() => void deleteGroup(record)}
                                >
                                  Xóa
                                </Button>
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  onClick={() => setConfirmDeleteGroupId(null)}
                                >
                                  Hủy
                                </Button>
                              </>
                            ) : (
                              <>
                                <Button
                                  size="sm"
                                  variant="outline"
                                  onClick={() => void loadGroupAccess(record)}
                                >
                                  Quyền truy cập
                                </Button>
                                <Button
                                  size="sm"
                                  variant="outline"
                                  onClick={() => {
                                    setEditingGroup(record);
                                    setEditingName(record.group.name);
                                  }}
                                >
                                  Đổi tên
                                </Button>
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  onClick={() =>
                                    setConfirmDeleteGroupId(record.group.id)
                                  }
                                >
                                  Xóa
                                </Button>
                              </>
                            )}
                          </div>
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
              <div className="flex items-center justify-between border-t border-border px-3 py-2 text-xs text-muted-foreground">
                <span>{groups.length} groups</span>
                <Badge variant="outline">Workspace-scoped</Badge>
              </div>
            </div>
            {accessGroup ? (
              <section
                className="space-y-3 rounded-md border border-border p-4"
                aria-labelledby="group-access-title"
              >
                <header className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <h3
                      id="group-access-title"
                      className="text-sm font-semibold"
                    >
                      Quyền truy cập · {accessGroup.group.name}
                    </h3>
                    <p className="mt-1 text-xs text-muted-foreground">
                      Quyền trực tiếp trên group này. API tiếp tục kiểm tra
                      action và workspace.
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => setAccessGroup(null)}
                  >
                    Đóng
                  </Button>
                </header>
                {accessError ? (
                  <p role="alert" className="text-xs text-destructive">
                    {accessError}
                  </p>
                ) : null}
                <div className="flex flex-wrap items-end gap-2">
                  <div className="min-w-52 flex-1 space-y-1">
                    <Label>Thành viên</Label>
                    <Select
                      value={shareMemberId}
                      onValueChange={setShareMemberId}
                      disabled={accessBusy}
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Chọn thành viên" />
                      </SelectTrigger>
                      <SelectContent>
                        {workspaceMembers.map((member) => (
                          <SelectItem key={member.id} value={member.id}>
                            {member.identity?.username ||
                              member.identity?.email ||
                              "Thành viên"}{" "}
                            · {member.role}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="w-40 space-y-1">
                    <Label>Quyền</Label>
                    <Select
                      value={shareAction}
                      onValueChange={(value) =>
                        setShareAction(value as GroupShareGrant["action"])
                      }
                      disabled={accessBusy}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {(
                          [
                            "read",
                            "update",
                            "execute",
                            "share",
                          ] as GroupShareGrant["action"][]
                        ).map((action) => (
                          <SelectItem key={action} value={action}>
                            {action}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <Button
                    size="sm"
                    onClick={() => void addGroupShare()}
                    disabled={accessBusy || !shareMemberId}
                  >
                    Cấp quyền
                  </Button>
                </div>
                {accessBusy ? (
                  <p role="status" className="text-xs text-muted-foreground">
                    Đang tải hoặc cập nhật quyền…
                  </p>
                ) : groupShares.filter((share) => !share.revokedAt).length ===
                  0 ? (
                  <p className="text-xs text-muted-foreground">
                    Chưa có quyền group trực tiếp.
                  </p>
                ) : (
                  <ul className="divide-y divide-border rounded-md border border-border">
                    {groupShares
                      .filter((share) => !share.revokedAt)
                      .map((share) => {
                        const member = workspaceMembers.find(
                          (item) => item.id === share.memberId,
                        );
                        return (
                          <li
                            key={share.id}
                            className="flex flex-wrap items-center gap-2 px-3 py-2 text-xs"
                          >
                            <span>
                              {member?.identity?.username ||
                                member?.identity?.email ||
                                "Thành viên"}
                            </span>
                            <Badge variant="outline">{share.action}</Badge>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="ml-auto"
                              disabled={accessBusy}
                              onClick={() => void revokeGroupShare(share)}
                            >
                              Thu hồi
                            </Button>
                          </li>
                        );
                      })}
                  </ul>
                )}
              </section>
            ) : null}
          </div>
        </section>
      ) : (
        <section className="min-h-0 flex-1 overflow-y-auto p-4 md:p-6">
          <div className="mx-auto grid max-w-5xl gap-3 md:grid-cols-2">
            <button
              type="button"
              onClick={() => setTab("members")}
              className="rounded-md border border-border p-4 text-left hover:bg-accent/40"
            >
              <span className="flex items-center gap-2 text-sm font-medium">
                <LuUsers className="size-4" />
                Thành viên & tài khoản
              </span>
              <p className="mt-1 text-xs text-muted-foreground">
                Tạo account, mời, đổi role hoặc đình chỉ membership của
                workspace này.
              </p>
            </button>
            <button
              type="button"
              onClick={() => setTab("groups")}
              className="rounded-md border border-border p-4 text-left hover:bg-accent/40"
            >
              <span className="flex items-center gap-2 text-sm font-medium">
                <LuFolder className="size-4" />
                Groups
              </span>
              <p className="mt-1 text-xs text-muted-foreground">
                Tạo, đổi tên và quản lý groups trong workspace.
              </p>
            </button>
            <button
              type="button"
              onClick={() => setTab("profiles")}
              className="rounded-md border border-border p-4 text-left hover:bg-accent/40"
            >
              <span className="flex items-center gap-2 text-sm font-medium">
                <LuLayoutGrid className="size-4" />
                Profiles & ungrouped
              </span>
              <p className="mt-1 text-xs text-muted-foreground">
                Tìm, lọc, phân trang, quản lý grants và gán/bỏ group bằng API.
              </p>
            </button>
          </div>
        </section>
      )}
    </div>
  );
}

function WorkspaceInvitePage({
  organizationName,
  workspaceId,
  workspaceName,
  onBack,
}: {
  organizationName: string;
  workspaceId: string;
  workspaceName: string;
  onBack: () => void;
}) {
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<MemberRole>("member");
  const [assignableRoles, setAssignableRoles] = useState<MemberRole[]>([]);
  const [accessLoading, setAccessLoading] = useState(true);
  const [accessError, setAccessError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [receipt, setReceipt] = useState<{
    code: string;
    link: string;
    expiresAt: number;
  } | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let disposed = false;
    void Promise.all([
      desktopApi.getAccount(),
      desktopApi.listMembers(workspaceId),
    ])
      .then(([account, page]) => {
        const own = page.items.find(
          (member) =>
            member.accountId === account.accountId &&
            member.status === "active",
        );
        const rank = account.platformAdmin
          ? Number.POSITIVE_INFINITY
          : account.managedWorkspaceIds.includes(workspaceId)
            ? MEMBER_ROLE_RANK.admin + 1
            : MEMBER_ROLE_RANK[own?.role ?? "member"];
        const roles = MEMBER_ROLES.filter(
          (candidate) => MEMBER_ROLE_RANK[candidate] < rank,
        );
        if (!disposed) {
          setAssignableRoles(roles);
          setRole(roles.at(-1) ?? "member");
        }
      })
      .catch((reason) => {
        if (!disposed) setAccessError(String(reason));
      })
      .finally(() => {
        if (!disposed) setAccessLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [workspaceId]);

  const createInvite = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!email.trim() || busy || !assignableRoles.includes(role)) return;
    setBusy(true);
    setError(null);
    setReceipt(null);
    try {
      const expiresAt = Math.floor(Date.now() / 1000) + 7 * 24 * 60 * 60;
      const created = await desktopApi.createMemberInvitation({
        workspaceId,
        email: email.trim(),
        role,
        expiresAt,
      });
      if (!created.code)
        throw new Error(
          "Lời mời đã tồn tại nhưng mã chỉ hiển thị một lần. Thu hồi lời mời cũ rồi tạo lời mời mới.",
        );
      setReceipt({
        code: `${created.invitationId}.${created.code}`,
        link: `buglogin://invite?invitationId=${encodeURIComponent(created.invitationId)}&code=${encodeURIComponent(created.code)}`,
        expiresAt: created.expiresAt,
      });
      showSuccessToast("Đã tạo lời mời cho workspace.");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      showErrorToast(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const copyInvite = async () => {
    if (!receipt) return;
    try {
      await navigator.clipboard.writeText(receipt.link);
      setCopied(true);
      showSuccessToast("Đã sao chép liên kết mời.");
      window.setTimeout(() => setCopied(false), 1800);
    } catch (reason) {
      setError(String(reason));
    }
  };

  return (
    <section className="min-h-0 flex-1 overflow-y-auto p-4 md:p-6">
      <div className="mx-auto max-w-2xl space-y-4">
        <header className="flex items-center gap-3 border-b border-border/60 pb-3">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onBack}
            aria-label="Quay lại thành viên"
          >
            <LuArrowLeft className="size-4" />
          </Button>
          <div>
            <p className="text-xs text-muted-foreground">
              Organizations / {organizationName} / {workspaceName}
            </p>
            <h2 className="text-base font-semibold">Mời thành viên</h2>
          </div>
        </header>
        {accessError ? (
          <p
            role="alert"
            className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
          >
            Không thể kiểm tra quyền workspace: {accessError}
          </p>
        ) : null}
        {!accessLoading && !accessError && assignableRoles.length === 0 ? (
          <p
            role="status"
            className="rounded-md border border-border p-3 text-sm text-muted-foreground"
          >
            Bạn không có quyền gán vai trò thấp hơn trong workspace này.
          </p>
        ) : null}
        <form
          onSubmit={(event) => void createInvite(event)}
          className="space-y-4 rounded-md border border-border p-4"
        >
          <p className="text-sm">
            Lời mời chỉ cấp membership cho <strong>{workspaceName}</strong>.
            Người nhận cần chấp nhận lời mời trước khi membership hoạt động.
          </p>
          <div className="space-y-1.5">
            <Label htmlFor="workspace-invite-email">Email người nhận</Label>
            <Input
              id="workspace-invite-email"
              type="email"
              autoComplete="off"
              maxLength={254}
              required
              value={email}
              onChange={(event) => setEmail(event.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="workspace-invite-role">
              Vai trò trong workspace
            </Label>
            <Select
              value={role}
              onValueChange={(value) => setRole(value as MemberRole)}
              disabled={
                accessLoading || !!accessError || assignableRoles.length === 0
              }
            >
              <SelectTrigger id="workspace-invite-role">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {assignableRoles.map((candidate) => (
                  <SelectItem key={candidate} value={candidate}>
                    {candidate}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {error ? (
            <p
              role="alert"
              className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
            >
              {error}
            </p>
          ) : null}
          {receipt ? (
            <section
              className="space-y-2 rounded-md border border-border bg-muted/20 p-3"
              aria-live="polite"
            >
              <p className="text-sm font-medium">Lời mời đã tạo</p>
              <p className="break-all font-mono text-xs">{receipt.link}</p>
              <p className="text-xs text-muted-foreground">
                Mã: {receipt.code}
              </p>
              <p className="text-xs text-muted-foreground">
                Hết hạn: {new Date(receipt.expiresAt * 1000).toLocaleString()}
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void copyInvite()}
              >
                {copied ? (
                  <LuCheck className="mr-1.5 size-3.5" />
                ) : (
                  <LuCopy className="mr-1.5 size-3.5" />
                )}
                {copied ? "Đã sao chép" : "Sao chép liên kết"}
              </Button>
            </section>
          ) : null}
          <div className="flex justify-end gap-2 border-t border-border/60 pt-3">
            <Button type="button" variant="outline" onClick={onBack}>
              Quay lại
            </Button>
            <Button
              type="submit"
              disabled={
                busy ||
                accessLoading ||
                !!accessError ||
                assignableRoles.length === 0
              }
            >
              {busy ? "Đang tạo…" : "Tạo lời mời"}
            </Button>
          </div>
        </form>
      </div>
    </section>
  );
}

"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuCheck,
  LuCopy,
  LuEllipsis,
  LuFolder,
  LuLayoutGrid,
  LuPlay,
  LuPlus,
  LuRefreshCw,
  LuSquare,
  LuUsers,
} from "react-icons/lu";
import { MemberManagementDialog } from "@/components/member-management-dialog";
import type { OrgWorkspaceRoute } from "@/components/org-workspace-route";
import { ResourceInventoryPanel } from "@/components/resource-inventory-panel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
  initialGroupId,
  initialProfileId,
  onRouteChange,
}: {
  organizationId: string;
  organizationName: string;
  workspaceId: string;
  workspaceName: string;
  onBack: () => void;
  initialTab?: WorkspaceTab;
  initialInvitePage?: boolean;
  initialGroupId?: string;
  initialProfileId?: string;
  onRouteChange?: (route: OrgWorkspaceRoute) => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<WorkspaceTab>(initialTab);
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const [groupsLoading, setGroupsLoading] = useState(false);
  const [groupsLoaded, setGroupsLoaded] = useState(false);
  const [groupsError, setGroupsError] = useState<string | null>(null);
  const [groupMutationError, setGroupMutationError] = useState<string | null>(
    null,
  );
  const [newGroupName, setNewGroupName] = useState("");
  const [createGroupOpen, setCreateGroupOpen] = useState(false);
  const [editingGroup, setEditingGroup] = useState<GroupRecord | null>(null);
  const [editingName, setEditingName] = useState("");
  const [confirmDeleteGroupId, setConfirmDeleteGroupId] = useState<
    string | null
  >(null);
  const [groupMutationBusy, setGroupMutationBusy] = useState(false);
  const [groupProfileOperation, setGroupProfileOperation] = useState<{
    groupId: string;
    action: "launch" | "stop";
    total: number;
    accepted: number;
    failed: number;
  } | null>(null);
  const [invitePageOpen, setInvitePageOpen] = useState(initialInvitePage);
  const [accessGroup, setAccessGroup] = useState<GroupRecord | null>(null);
  const [groupShares, setGroupShares] = useState<GroupShareGrant[]>([]);
  const [workspaceMembers, setWorkspaceMembers] = useState<Member[]>([]);
  const [shareMemberId, setShareMemberId] = useState("");
  const [shareAction, setShareAction] =
    useState<GroupShareGrant["action"]>("read");
  const [accessError, setAccessError] = useState<string | null>(null);
  const [accessBusy, setAccessBusy] = useState(false);
  const openedInitialGroup = useRef<string | null>(null);

  useEffect(() => setTab(initialTab), [initialTab]);
  useEffect(() => setInvitePageOpen(initialInvitePage), [initialInvitePage]);

  const loadGroups = useCallback(async () => {
    setGroupsLoading(true);
    setGroupsLoaded(false);
    setGroupsError(null);
    try {
      const response: unknown = await desktopApi.listGroups(workspaceId);
      if (Array.isArray(response)) {
        setGroups(response as GroupRecord[]);
      } else if (
        response &&
        typeof response === "object" &&
        "items" in response &&
        Array.isArray(response.items)
      ) {
        setGroups(response.items as GroupRecord[]);
      } else {
        throw new Error("Máy chủ trả về danh sách group không hợp lệ.");
      }
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setGroupsLoading(false);
      setGroupsLoaded(true);
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
    setGroupMutationError(null);
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
      setCreateGroupOpen(false);
      showSuccessToast("Đã tạo group.");
    } catch (error) {
      setGroupMutationError(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const saveGroupName = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!editingGroup || !editingName.trim() || groupMutationBusy) return;
    setGroupMutationBusy(true);
    setGroupMutationError(null);
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
      setGroupMutationError(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const deleteGroup = async (group: GroupRecord) => {
    if (groupMutationBusy) return;
    setGroupMutationBusy(true);
    setGroupMutationError(null);
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
      setGroupMutationError(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGroupMutationBusy(false);
    }
  };

  const runGroupProfiles = async (
    group: GroupRecord,
    action: "launch" | "stop",
  ) => {
    if (groupProfileOperation || groupMutationBusy) return;
    setGroupMutationError(null);
    setGroupProfileOperation({
      groupId: group.group.id,
      action,
      total: 0,
      accepted: 0,
      failed: 0,
    });
    try {
      const profileItems = [] as Awaited<
        ReturnType<typeof desktopApi.listProfiles>
      >["items"];
      const seenCursors = new Set<string>();
      let cursor: string | undefined;
      do {
        const page = await desktopApi.listProfiles(false, {
          limit: 100,
          workspaceId,
          ...(cursor ? { cursor } : {}),
        });
        profileItems.push(...page.items);
        const nextCursor = page.nextCursor ?? undefined;
        if (
          nextCursor &&
          (nextCursor.length > 512 || seenCursors.has(nextCursor))
        ) {
          throw new Error("Profile list cursor did not advance");
        }
        if (nextCursor) seenCursors.add(nextCursor);
        cursor = nextCursor;
      } while (cursor);
      const targets = profileItems.filter((profile) =>
        profile.groupIds.includes(group.group.id),
      );
      if (targets.length === 0) {
        setGroupMutationError(
          t("groups.noProfilesInGroup", {
            defaultValue: "Nhóm này chưa có hồ sơ để thao tác.",
          }),
        );
        return;
      }
      const operation = {
        groupId: group.group.id,
        action,
        total: targets.length,
        accepted: 0,
        failed: 0,
      };
      setGroupProfileOperation(operation);
      let accepted = 0;
      let failed = 0;
      const failureReasons: string[] = [];
      for (const profile of targets) {
        try {
          if (action === "launch") await desktopApi.launchProfile(profile.id);
          else await desktopApi.stopProfile(profile.id);
          accepted += 1;
        } catch (error) {
          failed += 1;
          failureReasons.push(
            error instanceof Error ? error.message : String(error),
          );
        }
        setGroupProfileOperation({ ...operation, accepted, failed });
      }
      if (failed > 0) {
        setGroupMutationError(
          `${t("groups.profileActionPartial", {
            accepted,
            failed,
            defaultValue: `Đã gửi ${accepted}/${targets.length} lệnh; ${failed} lệnh lỗi hoặc bị từ chối.`,
          })} ${[...new Set(failureReasons)].slice(0, 3).join(" · ")}`,
        );
      } else {
        showSuccessToast(
          t("groups.profileActionAccepted", {
            count: accepted,
            action: action === "launch" ? "mở" : "dừng",
            defaultValue: `Đã gửi yêu cầu ${action === "launch" ? "mở" : "dừng"} ${accepted} hồ sơ.`,
          }),
        );
      }
    } catch (error) {
      setGroupMutationError(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGroupProfileOperation(null);
    }
  };

  const loadGroupAccess = useCallback(
    async (record: GroupRecord) => {
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
    },
    [workspaceId],
  );

  useEffect(() => {
    if (!initialGroupId) {
      setAccessGroup(null);
      openedInitialGroup.current = null;
      return;
    }
    if (!groupsLoaded || groupsLoading || groupsError) return;
    if (openedInitialGroup.current === initialGroupId) return;
    const record = groups.find((item) => item.group.id === initialGroupId);
    if (record) {
      openedInitialGroup.current = initialGroupId;
      void loadGroupAccess(record);
    } else if (groups.length === 0) {
      onRouteChange?.({ organizationId, workspaceId, section: "groups" });
      showErrorToast(
        "Group trong liên kết không còn tồn tại trong workspace này.",
      );
    }
  }, [
    groups,
    groupsError,
    groupsLoaded,
    groupsLoading,
    initialGroupId,
    loadGroupAccess,
    onRouteChange,
    organizationId,
    workspaceId,
  ]);

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
            <nav
              aria-label="Breadcrumb"
              className="flex min-w-0 items-center gap-1 truncate text-xs text-muted-foreground"
            >
              <button
                type="button"
                className="truncate hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onClick={onBack}
              >
                Organizations
              </button>
              <span aria-hidden="true">/</span>
              <button
                type="button"
                className="truncate hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onClick={onBack}
              >
                {organizationName}
              </button>
              <span aria-hidden="true">/</span>
            </nav>
            <h1 className="truncate text-base font-semibold">
              {workspaceName}
            </h1>
          </div>
        </div>
        <nav
          aria-label="Workspace sections"
          className="flex shrink-0 gap-1 overflow-x-auto"
        >
          {tabs.map(({ id, label, icon: Icon }) => (
            <Button
              key={id}
              type="button"
              size="sm"
              variant={tab === id ? "secondary" : "ghost"}
              onClick={() => {
                setTab(id);
                setInvitePageOpen(false);
                setAccessGroup(null);
                onRouteChange?.({ organizationId, workspaceId, section: id });
              }}
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
            onClose={() => {
              setTab("overview");
              onRouteChange?.({
                organizationId,
                workspaceId,
                section: "overview",
              });
            }}
            onInvitePage={() => setInvitePageOpen(true)}
            organizationId={organizationId}
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
          initialResourceId={initialProfileId}
          onResourceRouteChange={(profileId) =>
            onRouteChange?.({
              organizationId,
              workspaceId,
              section: "profiles",
              profileId: profileId ?? undefined,
            })
          }
          showWorkspaceSelector={false}
        />
      ) : tab === "groups" ? (
        <section
          className="min-h-0 flex-1 overflow-y-auto p-3"
          aria-labelledby="workspace-groups-title"
        >
          <div className="space-y-3">
            <header className="flex min-h-9 items-center justify-between gap-2">
              <h2 id="workspace-groups-title" className="text-sm font-semibold">
                {t("groups.pageTitle", { defaultValue: "Nhóm" })}{" "}
                <span className="font-normal text-muted-foreground">
                  / {workspaceName}
                </span>
              </h2>
              <div className="flex shrink-0 items-center gap-1.5">
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => void loadGroups()}
                  disabled={groupsLoading}
                  aria-label={t("common.buttons.refresh", {
                    defaultValue: "Làm mới",
                  })}
                  title={t("common.buttons.refresh", {
                    defaultValue: "Làm mới",
                  })}
                  className="size-8"
                >
                  <LuRefreshCw
                    className={groupsLoading ? "animate-spin" : ""}
                  />
                </Button>
                <Button
                  size="sm"
                  onClick={() => setCreateGroupOpen((open) => !open)}
                  aria-expanded={createGroupOpen}
                  className="h-8"
                >
                  <LuPlus className="size-3.5" />
                  {t("common.buttons.create", { defaultValue: "Tạo mới" })}
                </Button>
              </div>
            </header>

            {createGroupOpen && (
              <form
                onSubmit={(event) => void createGroup(event)}
                className="flex items-end gap-2 rounded-md border border-border bg-card p-2"
              >
                <div className="min-w-52 flex-1 space-y-1">
                  <Label htmlFor="new-workspace-group">
                    {t("groups.name", { defaultValue: "Tên nhóm" })}
                  </Label>
                  <Input
                    id="new-workspace-group"
                    value={newGroupName}
                    onChange={(event) => setNewGroupName(event.target.value)}
                    maxLength={80}
                    required
                    placeholder={t("groups.name", { defaultValue: "Tên nhóm" })}
                    autoFocus
                    className="h-8"
                  />
                </div>
                <Button
                  size="sm"
                  type="submit"
                  disabled={!newGroupName.trim() || groupMutationBusy}
                >
                  {groupMutationBusy
                    ? t("common.buttons.creating", {
                        defaultValue: "Đang tạo…",
                      })
                    : t("common.buttons.create", { defaultValue: "Tạo nhóm" })}
                </Button>
                <Button
                  size="sm"
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setCreateGroupOpen(false);
                    setNewGroupName("");
                  }}
                >
                  {t("common.buttons.cancel", { defaultValue: "Hủy" })}
                </Button>
              </form>
            )}

            {groupsError && (
              <div
                role="alert"
                className="flex items-center justify-between gap-3 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>
                  {t("groups.loadFailed", {
                    defaultValue: "Không tải được danh sách nhóm.",
                  })}{" "}
                  {groupsError}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void loadGroups()}
                  disabled={groupsLoading}
                >
                  {t("common.buttons.retry", { defaultValue: "Thử lại" })}
                </Button>
              </div>
            )}
            {groupMutationError && (
              <p
                role="alert"
                className="rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                {t("groups.actionFailed", {
                  defaultValue: "Không thể hoàn tất thao tác nhóm.",
                })}{" "}
                {groupMutationError}
              </p>
            )}
            {groupProfileOperation && (
              <p role="status" className="text-xs text-muted-foreground">
                {groupProfileOperation.total === 0
                  ? t("groups.profileActionLoading", {
                      defaultValue: "Đang tải profile trong nhóm…",
                    })
                  : t("groups.profileActionProgress", {
                      accepted: groupProfileOperation.accepted,
                      total: groupProfileOperation.total,
                      defaultValue: `Đã gửi ${groupProfileOperation.accepted}/${groupProfileOperation.total} lệnh profile.`,
                    })}
              </p>
            )}
            <div className="overflow-hidden rounded-md border border-border">
              <table className="w-full text-left text-xs">
                <thead className="sticky top-0 z-10 bg-muted/60 text-xs text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2">Tên nhóm</th>
                    <th className="px-3 py-2">Cập nhật</th>
                    <th className="w-16 px-3 py-2 text-right">Thao tác</th>
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
                      <tr
                        key={record.group.id}
                        className="h-10 hover:bg-muted/30"
                      >
                        <td className="px-3 py-1.5">
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
                        <td className="px-3 py-1.5 text-xs text-muted-foreground">
                          {new Date(record.group.updatedAt).toLocaleString()}
                        </td>
                        <td className="px-3 py-1.5">
                          <div className="flex justify-end gap-1">
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
                              <DropdownMenu>
                                <DropdownMenuTrigger asChild>
                                  <Button
                                    type="button"
                                    size="icon"
                                    variant="ghost"
                                    className="size-7"
                                    aria-label={t("groups.actionsFor", {
                                      name: record.group.name,
                                      defaultValue: `Thao tác với nhóm ${record.group.name}`,
                                    })}
                                  >
                                    <LuEllipsis className="size-4" />
                                  </Button>
                                </DropdownMenuTrigger>
                                <DropdownMenuContent
                                  align="end"
                                  className="w-48"
                                >
                                  <DropdownMenuItem
                                    onSelect={() => {
                                      openedInitialGroup.current =
                                        record.group.id;
                                      void loadGroupAccess(record);
                                      onRouteChange?.({
                                        organizationId,
                                        workspaceId,
                                        section: "groups",
                                        groupId: record.group.id,
                                      });
                                    }}
                                  >
                                    {t("groups.access", {
                                      defaultValue: "Quyền truy cập",
                                    })}
                                  </DropdownMenuItem>
                                  <DropdownMenuItem
                                    disabled={Boolean(groupProfileOperation)}
                                    onSelect={() =>
                                      void runGroupProfiles(record, "launch")
                                    }
                                  >
                                    <LuPlay />
                                    {t("groups.runProfiles", {
                                      defaultValue: "Mở hồ sơ trong nhóm",
                                    })}
                                  </DropdownMenuItem>
                                  <DropdownMenuItem
                                    disabled={Boolean(groupProfileOperation)}
                                    onSelect={() =>
                                      void runGroupProfiles(record, "stop")
                                    }
                                  >
                                    <LuSquare />
                                    {t("groups.stopProfiles", {
                                      defaultValue: "Dừng hồ sơ trong nhóm",
                                    })}
                                  </DropdownMenuItem>
                                  <DropdownMenuItem
                                    onSelect={() => {
                                      setEditingGroup(record);
                                      setEditingName(record.group.name);
                                    }}
                                  >
                                    {t("common.buttons.rename", {
                                      defaultValue: "Đổi tên",
                                    })}
                                  </DropdownMenuItem>
                                  <DropdownMenuSeparator />
                                  <DropdownMenuItem
                                    variant="destructive"
                                    onSelect={() =>
                                      setConfirmDeleteGroupId(record.group.id)
                                    }
                                  >
                                    {t("common.buttons.delete", {
                                      defaultValue: "Xóa nhóm",
                                    })}
                                  </DropdownMenuItem>
                                </DropdownMenuContent>
                              </DropdownMenu>
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
                    onClick={() => {
                      setAccessGroup(null);
                      onRouteChange?.({
                        organizationId,
                        workspaceId,
                        section: "groups",
                      });
                    }}
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
          <div className="grid w-full gap-3 sm:grid-cols-2 xl:grid-cols-3">
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

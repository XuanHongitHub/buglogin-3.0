"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuChevronLeft, LuChevronRight, LuRefreshCw } from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type {
  GrantAction,
  GrantEffect,
  Organization,
  ResourceGrant,
  Workspace,
} from "../../apps/desktop/src/contracts";
import {
  appendResourceCursorPage,
  RESOURCE_INVENTORY_PAGE_SIZE,
  type ResourceCursorPage,
  resourceListPageInput,
} from "../../apps/desktop/src/resource-inventory-pagination";
import type {
  GroupRecord,
  Member,
  ProfileRecord,
  Resource,
} from "../../frontend/src/generated/command-registry";

const desktopApi = createTauriDesktopApi();

export function ResourceInventoryPanel() {
  const { t } = useTranslation();
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [workspaceId, setWorkspaceId] = useState("");
  const [resources, setResources] = useState<Resource[]>([]);
  const [pageStack, setPageStack] = useState<ResourceCursorPage[]>([
    { cursor: undefined },
  ]);
  const [pageIndex, setPageIndex] = useState(0);
  const [selected, setSelected] = useState<Resource | null>(null);
  const [selectedProfile, setSelectedProfile] = useState<ProfileRecord | null>(
    null,
  );
  const [draftGroupIds, setDraftGroupIds] = useState<string[]>([]);
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const [isLoadingGroups, setIsLoadingGroups] = useState(false);
  const [isSavingGroups, setIsSavingGroups] = useState(false);
  const [groupsError, setGroupsError] = useState<string | null>(null);
  const [selectedGrants, setSelectedGrants] = useState<
    Awaited<ReturnType<typeof desktopApi.listGrants>>["items"]
  >([]);
  const [members, setMembers] = useState<Member[]>([]);
  const [grantMemberId, setGrantMemberId] = useState("");
  const [grantAction, setGrantAction] = useState<GrantAction>("read");
  const [grantEffect, setGrantEffect] = useState<GrantEffect>("allow");
  const [isSavingGrant, setIsSavingGrant] = useState(false);
  const [grantError, setGrantError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("all");
  const [state, setState] = useState("all");
  const [isLoadingWorkspaces, setIsLoadingWorkspaces] = useState(true);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingDetail, setIsLoadingDetail] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [hasNextPage, setHasNextPage] = useState(false);
  const [currentNextCursor, setCurrentNextCursor] = useState<string | null>(
    null,
  );
  const requestRef = useRef(0);
  const detailRequestRef = useRef(0);

  useEffect(() => {
    let disposed = false;
    const load = async () => {
      setIsLoadingWorkspaces(true);
      try {
        const orgs = await desktopApi.listOrganizations();
        const pages = await Promise.all(
          orgs.items.map((org: Organization) =>
            desktopApi.listWorkspaces(org.id),
          ),
        );
        const all = pages.flatMap((page) => page.items);
        if (disposed) return;
        setWorkspaces(all);
        setWorkspaceId((current) =>
          all.some((workspace) => workspace.id === current)
            ? current
            : (all[0]?.id ?? ""),
        );
      } catch (error) {
        if (!disposed)
          setLoadError(error instanceof Error ? error.message : String(error));
      } finally {
        if (!disposed) setIsLoadingWorkspaces(false);
      }
    };
    void load();
    return () => {
      disposed = true;
    };
  }, []);

  const loadPage = useCallback(
    async (id: string, index: number, stack: ResourceCursorPage[]) => {
      const request = ++requestRef.current;
      if (!id) {
        setResources([]);
        setHasNextPage(false);
        setIsLoading(false);
        return;
      }
      setIsLoading(true);
      try {
        const page = await desktopApi.listResources(
          resourceListPageInput(id, stack[index]?.cursor),
        );
        if (request !== requestRef.current) return;
        setResources(page.items);
        setCurrentNextCursor(page.nextCursor ?? null);
        setHasNextPage(Boolean(page.nextCursor));
        setLoadError(null);
      } catch (error) {
        if (request === requestRef.current)
          setLoadError(error instanceof Error ? error.message : String(error));
      } finally {
        if (request === requestRef.current) setIsLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    setPageStack([{ cursor: undefined }]);
    setPageIndex(0);
    detailRequestRef.current += 1;
    setSelected(null);
    setResources([]);
    setLoadError(null);
    setHasNextPage(false);
    setSearch("");
    void loadPage(workspaceId, 0, [{ cursor: undefined }]);
    return () => {
      requestRef.current += 1;
    };
  }, [workspaceId, loadPage]);

  useEffect(() => {
    let disposed = false;
    setIsLoadingGroups(Boolean(workspaceId));
    setGroups([]);
    setGroupsError(null);
    if (!workspaceId) return;
    void desktopApi
      .listGroups(workspaceId)
      .then((items) => {
        if (!disposed) setGroups(items);
      })
      .catch((error) => {
        if (!disposed)
          setGroupsError(
            error instanceof Error ? error.message : String(error),
          );
      })
      .finally(() => {
        if (!disposed) setIsLoadingGroups(false);
      });
    return () => {
      disposed = true;
    };
  }, [workspaceId]);

  const refresh = () => void loadPage(workspaceId, pageIndex, pageStack);

  const goNext = async () => {
    const current = pageStack[pageIndex];
    if (!workspaceId || !hasNextPage || !current || !currentNextCursor) return;
    const nextStack = appendResourceCursorPage(
      pageStack,
      pageIndex,
      currentNextCursor,
    );
    if (!nextStack) {
      setLoadError(
        "API pagination cursor repeated; stopped to avoid looping over the same resource page.",
      );
      setHasNextPage(false);
      return;
    }
    const request = ++requestRef.current;
    setIsLoading(true);
    try {
      const page = await desktopApi.listResources(
        resourceListPageInput(workspaceId, currentNextCursor),
      );
      if (request !== requestRef.current) return;
      setPageStack(nextStack);
      setPageIndex(pageIndex + 1);
      setResources(page.items);
      setCurrentNextCursor(page.nextCursor ?? null);
      setHasNextPage(Boolean(page.nextCursor));
      setLoadError(null);
    } catch (error) {
      if (request === requestRef.current)
        setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      if (request === requestRef.current) setIsLoading(false);
    }
  };

  const goPrevious = async () => {
    if (pageIndex === 0) return;
    const previous = pageIndex - 1;
    setPageIndex(previous);
    await loadPage(workspaceId, previous, pageStack);
  };

  const openDetail = async (resource: Resource) => {
    setSelected(resource);
    setSelectedProfile(null);
    setDraftGroupIds([]);
    setIsLoadingDetail(true);
    setDetailError(null);
    setGrantError(null);
    setSelectedGrants([]);
    const request = ++detailRequestRef.current;
    try {
      const [detail, grants, members, profile] = await Promise.all([
        desktopApi.getResource({ workspaceId, resourceId: resource.id }),
        desktopApi.listGrants(workspaceId, resource.id),
        desktopApi.listMembers(workspaceId),
        resource.kind === "profile"
          ? desktopApi.getProfile({ profileId: resource.id })
          : Promise.resolve(null),
      ]);
      if (request !== detailRequestRef.current) return;
      setSelected(detail);
      setSelectedProfile(profile);
      setDraftGroupIds(profile?.groupIds ?? []);
      setMembers(members.items);
      setGrantMemberId(
        members.items.find(
          (member) => member.status === "active" && member.identity,
        )?.id ?? "",
      );
      setSelectedGrants(grants.items);
    } catch (error) {
      if (request === detailRequestRef.current)
        setDetailError(error instanceof Error ? error.message : String(error));
    } finally {
      if (request === detailRequestRef.current) setIsLoadingDetail(false);
    }
  };

  const saveProfileGroups = async () => {
    if (!workspaceId || !selectedProfile || groupsError || isLoadingGroups)
      return;
    setIsSavingGroups(true);
    setGroupsError(null);
    try {
      const updated = await desktopApi.replaceProfileGroups({
        workspaceId,
        profileId: selectedProfile.id,
        expectedRevision: selectedProfile.revision,
        groupIds: draftGroupIds,
      });
      setSelectedProfile(updated);
      setDraftGroupIds(updated.groupIds);
      setSelected((current) =>
        current?.id === updated.id
          ? { ...current, displayName: updated.name }
          : current,
      );
      void loadPage(workspaceId, pageIndex, pageStack);
    } catch (error) {
      setGroupsError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSavingGroups(false);
    }
  };

  const addDirectGrant = async () => {
    if (!workspaceId || !selected || !grantMemberId) return;
    const duplicate = selectedGrants.some(
      (grant) =>
        grant.memberId === grantMemberId &&
        grant.action === grantAction &&
        grant.effect === grantEffect,
    );
    if (duplicate) {
      setGrantError("Quyền này đã được gán trực tiếp cho thành viên.");
      return;
    }
    setIsSavingGrant(true);
    setGrantError(null);
    const grant: ResourceGrant = {
      id: `grt_${crypto.randomUUID().replaceAll("-", "")}`,
      workspaceId,
      resourceId: selected.id,
      memberId: grantMemberId,
      action: grantAction,
      effect: grantEffect,
    };
    try {
      const created = await desktopApi.createGrant(grant);
      setSelectedGrants((current) => [...current, created]);
    } catch (error) {
      setGrantError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSavingGrant(false);
    }
  };

  const revokeDirectGrant = async (grantId: string) => {
    setIsSavingGrant(true);
    setGrantError(null);
    try {
      const result = await desktopApi.revokeGrant(grantId);
      if (result.revoked)
        setSelectedGrants((current) =>
          current.filter((grant) => grant.id !== grantId),
        );
    } catch (error) {
      setGrantError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSavingGrant(false);
    }
  };

  const visibleResources = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return resources.filter(
      (resource) =>
        (kind === "all" || resource.kind === kind) &&
        (state === "all" || resource.state === state) &&
        (!query || resource.displayName.toLocaleLowerCase().includes(query)),
    );
  }, [resources, search, kind, state]);

  const selectedWorkspace = workspaces.find(
    (workspace) => workspace.id === workspaceId,
  );
  const grantableMembers = useMemo(
    () =>
      members.filter(
        (member) => member.status === "active" && Boolean(member.identity),
      ),
    [members],
  );

  return (
    <section
      className="flex min-h-0 flex-1 flex-col gap-3 p-4"
      aria-labelledby="resource-inventory-title"
    >
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        <div>
          <h2 id="resource-inventory-title" className="text-base font-semibold">
            {t("resourceInventory.title", { defaultValue: "Tài nguyên" })}
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">
            Mỗi lần chỉ tải một trang tối đa {RESOURCE_INVENTORY_PAGE_SIZE} tài
            nguyên.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Select value={workspaceId} onValueChange={setWorkspaceId}>
            <SelectTrigger className="h-9 w-56" aria-label="Workspace">
              <SelectValue placeholder="Chọn workspace" />
            </SelectTrigger>
            <SelectContent>
              {workspaces.map((workspace) => (
                <SelectItem key={workspace.id} value={workspace.id}>
                  {workspace.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            size="sm"
            onClick={refresh}
            disabled={!workspaceId || isLoading}
            aria-label="Tải lại"
          >
            <LuRefreshCw
              className={
                isLoading ? "mr-2 size-3.5 animate-spin" : "mr-2 size-3.5"
              }
            />
            Tải lại
          </Button>
        </div>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <Input
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Tìm trong trang hiện tại…"
          className="h-9 min-w-52 flex-1"
          aria-label="Tìm trong trang hiện tại"
        />
        <Select value={kind} onValueChange={setKind}>
          <SelectTrigger className="h-9 w-40">
            <SelectValue placeholder="Loại" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Mọi loại</SelectItem>
            {[
              "profile",
              "proxy",
              "vpn",
              "cookieset",
              "extension",
              "runtime",
            ].map((value) => (
              <SelectItem key={value} value={value}>
                {value}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={state} onValueChange={setState}>
          <SelectTrigger className="h-9 w-40">
            <SelectValue placeholder="Trạng thái" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Mọi trạng thái</SelectItem>
            {["active", "archived", "locked"].map((value) => (
              <SelectItem key={value} value={value}>
                {value}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Badge variant="outline">
          {visibleResources.length} / {resources.length} trong trang
        </Badge>
      </div>

      {loadError && (
        <div
          role="alert"
          className="flex items-center justify-between rounded-md border border-destructive/30 p-3 text-sm text-destructive"
        >
          <span>{loadError}</span>
          <Button variant="outline" size="sm" onClick={refresh}>
            Thử lại
          </Button>
        </div>
      )}
      {isLoadingWorkspaces ? (
        <p
          role="status"
          className="p-6 text-center text-sm text-muted-foreground"
        >
          Đang tải workspace…
        </p>
      ) : !workspaceId ? (
        <p className="p-6 text-center text-sm text-muted-foreground">
          Không có workspace nào khả dụng.
        </p>
      ) : null}

      <div className="min-h-0 flex-1 overflow-auto rounded-md border border-border">
        <table className="w-full border-collapse text-left text-sm">
          <thead className="sticky top-0 z-10 border-b bg-muted/70 text-xs text-muted-foreground">
            <tr>
              <th className="p-2.5">Tài nguyên</th>
              <th className="p-2.5">Loại</th>
              <th className="p-2.5">Trạng thái</th>
              <th className="p-2.5">Workspace</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border/60">
            {visibleResources.map((resource) => (
              <tr
                key={resource.id}
                className={
                  selected?.id === resource.id
                    ? "bg-accent/50"
                    : "hover:bg-accent/30"
                }
              >
                <td className="p-2">
                  <button
                    type="button"
                    className="rounded px-1 text-left font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={() => void openDetail(resource)}
                  >
                    {resource.displayName ||
                      t("resourceInventory.unnamed", {
                        defaultValue: "Chưa đặt tên",
                      })}
                  </button>
                </td>
                <td className="p-2">
                  <Badge variant="outline">{resource.kind}</Badge>
                </td>
                <td className="p-2">
                  <Badge
                    variant={
                      resource.state === "locked" ? "destructive" : "secondary"
                    }
                  >
                    {resource.state}
                  </Badge>
                </td>
                <td className="p-2 text-muted-foreground">
                  {selectedWorkspace?.name ?? "—"}
                </td>
              </tr>
            ))}
            {!isLoading &&
              resources.length > 0 &&
              visibleResources.length === 0 && (
                <tr>
                  <td
                    colSpan={4}
                    className="p-8 text-center text-muted-foreground"
                  >
                    Không có tài nguyên khớp bộ lọc trong trang này.
                  </td>
                </tr>
              )}
            {!isLoading &&
              resources.length === 0 &&
              !loadError &&
              workspaceId && (
                <tr>
                  <td
                    colSpan={4}
                    className="p-8 text-center text-muted-foreground"
                  >
                    Workspace này chưa có tài nguyên.
                  </td>
                </tr>
              )}
            {isLoading && resources.length === 0 && (
              <tr>
                <td
                  colSpan={4}
                  role="status"
                  className="p-8 text-center text-muted-foreground"
                >
                  Đang tải tài nguyên…
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <footer className="flex items-center justify-between text-xs text-muted-foreground">
        <span>
          {selectedWorkspace?.name ?? ""} · Trang {pageIndex + 1}
        </span>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void goPrevious()}
            disabled={pageIndex === 0 || isLoading}
          >
            <LuChevronLeft className="mr-1 size-3.5" />
            Trước
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void goNext()}
            disabled={!hasNextPage || isLoading}
          >
            Tiếp
            <LuChevronRight className="ml-1 size-3.5" />
          </Button>
        </div>
      </footer>

      {selected && (
        <aside
          className="max-h-64 overflow-auto rounded-md border border-border p-3"
          aria-label="Chi tiết tài nguyên"
        >
          <div className="flex items-start justify-between gap-2">
            <div>
              <h3 className="text-sm font-semibold">
                {selected.displayName || "Chưa đặt tên"}
              </h3>
              <p className="text-xs text-muted-foreground">
                {selected.kind} · {selected.state} · {selectedWorkspace?.name}
              </p>
            </div>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                detailRequestRef.current += 1;
                setSelected(null);
              }}
            >
              Đóng
            </Button>
          </div>
          <h4 className="mt-3 text-xs font-semibold">Quyền trực tiếp</h4>
          <div className="mt-2 grid gap-2 rounded-md border border-border p-2.5 sm:grid-cols-[minmax(180px,1.3fr)_minmax(120px,0.8fr)_minmax(100px,0.6fr)_auto]">
            <Select
              value={grantMemberId}
              onValueChange={setGrantMemberId}
              disabled={grantableMembers.length === 0 || isSavingGrant}
            >
              <SelectTrigger className="h-8 text-xs" aria-label="Thành viên">
                <SelectValue placeholder="Chọn thành viên" />
              </SelectTrigger>
              <SelectContent>
                {grantableMembers.map((member) => (
                  <SelectItem key={member.id} value={member.id}>
                    {member.identity?.username} · {member.identity?.email} ·{" "}
                    {member.role}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select
              value={grantAction}
              onValueChange={(value) => setGrantAction(value as GrantAction)}
              disabled={isSavingGrant}
            >
              <SelectTrigger className="h-8 text-xs" aria-label="Quyền">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {[
                  "read",
                  "create",
                  "update",
                  "delete",
                  "execute",
                  "manage",
                ].map((action) => (
                  <SelectItem key={action} value={action}>
                    {action}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select
              value={grantEffect}
              onValueChange={(value) => setGrantEffect(value as GrantEffect)}
              disabled={isSavingGrant}
            >
              <SelectTrigger className="h-8 text-xs" aria-label="Hiệu lực">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="allow">Cho phép</SelectItem>
                <SelectItem value="deny">Từ chối</SelectItem>
              </SelectContent>
            </Select>
            <Button
              size="sm"
              className="h-8"
              onClick={() => void addDirectGrant()}
              disabled={!grantMemberId || isSavingGrant}
            >
              {isSavingGrant ? "Đang lưu…" : "Gán quyền"}
            </Button>
          </div>
          {grantableMembers.length === 0 ? (
            <p className="mt-1 text-[11px] text-muted-foreground">
              Chỉ thành viên đang hoạt động có danh tính mà bạn được phép xem
              mới xuất hiện để gán quyền.
            </p>
          ) : null}
          {grantError ? (
            <p role="alert" className="mt-1 text-xs text-destructive">
              {grantError}
            </p>
          ) : null}
          {isLoadingDetail ? (
            <p role="status" className="mt-2 text-xs text-muted-foreground">
              Đang tải quyền…
            </p>
          ) : detailError ? (
            <p role="alert" className="mt-2 text-xs text-destructive">
              {detailError}
            </p>
          ) : selectedGrants.length === 0 ? (
            <p className="mt-2 text-xs text-muted-foreground">
              API không trả grant trực tiếp nào cho tài nguyên này.
            </p>
          ) : (
            <ul className="mt-2 space-y-1">
              {selectedGrants.map((grant) => {
                const member = members.find(
                  (item) => item.id === grant.memberId,
                );
                return (
                  <li
                    key={grant.id}
                    className="flex flex-wrap items-center gap-2 text-xs"
                  >
                    <span>
                      {member?.identity
                        ? `${member.identity.username} · ${member.identity.email} · ${member.role}`
                        : "Thành viên không hiển thị"}
                    </span>
                    <Badge variant="outline">{grant.action}</Badge>
                    <Badge
                      variant={
                        grant.effect === "deny" ? "destructive" : "secondary"
                      }
                    >
                      {grant.effect}
                    </Badge>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="ml-auto h-7 px-2 text-xs"
                      onClick={() => void revokeDirectGrant(grant.id)}
                      disabled={isSavingGrant}
                    >
                      Thu hồi
                    </Button>
                  </li>
                );
              })}
            </ul>
          )}
          <p className="mt-2 text-[11px] text-muted-foreground">
            Quyền kế thừa từ team và nguồn hình thành effective access chưa được
            API cung cấp. Di chuyển khác workspace và thao tác hàng loạt cũng
            chưa có API.
          </p>
          {selected.kind === "profile" ? (
            <section className="mt-4 border-t border-border pt-3">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <h4 className="text-xs font-semibold">Nhóm profile</h4>
                  <p className="mt-1 text-[11px] text-muted-foreground">
                    Có thể chọn nhiều nhóm. Bỏ chọn hết để profile không thuộc
                    nhóm nào.
                  </p>
                </div>
                <Button
                  size="sm"
                  onClick={() => void saveProfileGroups()}
                  disabled={
                    !selectedProfile ||
                    isSavingGroups ||
                    isLoadingGroups ||
                    Boolean(groupsError) ||
                    JSON.stringify([...draftGroupIds].sort()) ===
                      JSON.stringify(
                        [...(selectedProfile?.groupIds ?? [])].sort(),
                      )
                  }
                >
                  {isSavingGroups ? "Đang lưu…" : "Lưu nhóm"}
                </Button>
              </div>
              {groupsError ? (
                <p role="alert" className="mt-2 text-xs text-destructive">
                  Không thể tải hoặc lưu nhóm: {groupsError}
                </p>
              ) : null}
              {isLoadingGroups ? (
                <p role="status" className="mt-2 text-xs text-muted-foreground">
                  Đang tải nhóm…
                </p>
              ) : groups.length === 0 ? (
                <p className="mt-2 text-xs text-muted-foreground">
                  Workspace chưa có nhóm. Lưu danh sách rỗng sẽ bỏ profile khỏi
                  mọi nhóm.
                </p>
              ) : (
                <ul className="mt-2 grid gap-2 sm:grid-cols-2">
                  {groups.map(({ group }) => {
                    const checked = draftGroupIds.includes(group.id);
                    return (
                      <li key={group.id}>
                        <div className="flex items-center gap-2 rounded-md border border-border px-2.5 py-2 text-xs hover:bg-accent/40">
                          <Checkbox
                            aria-label={`Gán profile vào nhóm ${group.name}`}
                            checked={checked}
                            disabled={!selectedProfile || isSavingGroups}
                            onCheckedChange={(value) =>
                              setDraftGroupIds((current) =>
                                value === true
                                  ? current.includes(group.id)
                                    ? current
                                    : [...current, group.id]
                                  : current.filter((id) => id !== group.id),
                              )
                            }
                          />
                          <span>{group.name}</span>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              )}
            </section>
          ) : null}
        </aside>
      )}
    </section>
  );
}

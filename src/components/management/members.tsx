"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuChevronDown,
  LuCopy,
  LuEllipsis,
  LuMail,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuShield,
  LuTrash2,
  LuUserPlus,
  LuUsers,
  LuX,
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
import { showSuccessToast } from "@/lib/toast-utils";
import type {
  AccountView,
  GroupRecord,
  GroupShareGrant,
  Member,
  MemberInvitationCreateReceipt,
  MemberRole,
  OrganizationMemberView,
} from "../../../frontend/src/generated/command-registry";
import {
  AccountError,
  AccountProvisioning,
  accountError,
  managementAccountApi as api,
  RolePicker,
} from "./accounts";
import "./accounts-i18n";

const th = "px-3 py-2 text-left text-xs font-medium text-muted-foreground";
const td = "px-3 py-2.5 text-sm align-middle";
type MemberResult = { label: string; failed: boolean; message: string };

export function WorkspaceMembers({
  organizationId,
  workspaceId,
  onDataChanged,
}: {
  organizationId: string;
  workspaceId: string;
  onDataChanged?: () => void;
}) {
  const { t } = useTranslation("managementAccounts");
  const [members, setMembers] = useState<Member[]>([]);
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const [shares, setShares] = useState<GroupShareGrant[]>([]);
  const [actor, setActor] = useState<AccountView>();
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [roleFilter, setRoleFilter] = useState("all");
  const [selected, setSelected] = useState<string[]>([]);
  const [provisioning, setProvisioning] = useState(false);
  const [editing, setEditing] = useState<Member[]>([]);
  const [editRole, setEditRole] = useState<MemberRole>("member");
  const [changeRole, setChangeRole] = useState(true);
  const [replaceGroups, setReplaceGroups] = useState(false);
  const [selectedGroups, setSelectedGroups] = useState<string[]>([]);
  const [operation, setOperation] = useState<
    "activate" | "suspend" | "remove"
  >();
  const [outcomes, setOutcomes] = useState<MemberResult[]>([]);
  const [adding, setAdding] = useState(false);
  const [directory, setDirectory] = useState<OrganizationMemberView[]>([]);
  const [addQuery, setAddQuery] = useState("");
  const [addSelected, setAddSelected] = useState<string[]>([]);
  const [addRole, setAddRole] = useState<MemberRole>("member");
  const [inviting, setInviting] = useState(false);
  const [inviteText, setInviteText] = useState("");
  const [inviteRole, setInviteRole] = useState<MemberRole>("member");
  const [invitationResults, setInvitationResults] = useState<
    (MemberResult & { receipt?: MemberInvitationCreateReceipt })[]
  >([]);

  const reload = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [memberPage, groupItems, viewer] = await Promise.all([
        api.listMembers(workspaceId),
        api.listGroups(workspaceId),
        api.getAccount(),
      ]);
      setMembers(memberPage.items);
      setGroups(groupItems);
      setActor(viewer);
      const grantResults = await Promise.allSettled(
        groupItems.map((group) =>
          api.listGroupShares({ workspaceId, groupId: group.group.id }),
        ),
      );
      setShares(
        grantResults.flatMap((result) =>
          result.status === "fulfilled" ? result.value : [],
        ),
      );
      if (grantResults.some((result) => result.status === "rejected"))
        setError(t("partial"));
      setSelected([]);
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setLoading(false);
    }
  }, [workspaceId, t]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const visible = useMemo(
    () =>
      members.filter(
        (member) =>
          `${member.identity?.username ?? ""} ${member.identity?.email ?? ""}`
            .toLowerCase()
            .includes(query.toLowerCase()) &&
          (roleFilter === "all" || member.role === roleFilter),
      ),
    [members, query, roleFilter],
  );
  const chosen = members.filter((member) => selected.includes(member.id));
  const label = (member: Member) =>
    member.identity?.username || member.identity?.email || t("members");
  const effectiveGroups = (member: Member) =>
    member.role === "owner" || member.role === "admin"
      ? t("inherited")
      : groups
          .filter((group) =>
            shares.some(
              (share) =>
                share.groupId === group.group.id &&
                share.memberId === member.id &&
                share.effect === "allow" &&
                !share.revokedAt,
            ),
          )
          .map((group) => group.group.name)
          .join(", ") || "—";
  const mayManage =
    actor?.platformAdmin ||
    actor?.managedWorkspaceIds.includes(workspaceId) ||
    members.some(
      (member) =>
        member.accountId === actor?.accountId &&
        member.status === "active" &&
        ["owner", "admin", "lead"].includes(member.role),
    );

  function openEdit(items: Member[], groupsOnly = false) {
    setEditing(items);
    setEditRole(items.length === 1 ? items[0].role : "member");
    setChangeRole(!groupsOnly);
    setReplaceGroups(groupsOnly);
    setSelectedGroups(
      items.length === 1
        ? [
            ...new Set(
              shares
                .filter(
                  (share) =>
                    share.memberId === items[0].id &&
                    share.effect === "allow" &&
                    !share.revokedAt,
                )
                .map((share) => share.groupId),
            ),
          ]
        : [],
    );
    setOutcomes([]);
    setError("");
  }

  async function editAccess() {
    setPending(true);
    setError("");
    try {
      const items: {
        key: string;
        commandId: string;
        input: Record<string, unknown>;
      }[] = [];
      for (const member of editing) {
        if (changeRole && member.role !== editRole)
          items.push({
            key: `${member.id}:role`,
            commandId: "member.role.update",
            input: { workspaceId, memberId: member.id, role: editRole },
          });
        if (replaceGroups) {
          const direct = shares.filter(
            (share) =>
              share.memberId === member.id &&
              share.effect === "allow" &&
              !share.revokedAt,
          );
          for (const share of direct)
            if (!selectedGroups.includes(share.groupId))
              items.push({
                key: `${member.id}:revoke:${share.id}`,
                commandId: "group.share.revoke",
                input: {
                  workspaceId,
                  groupId: share.groupId,
                  grantId: share.id,
                  expectedRevision: share.revision,
                },
              });
          for (const groupId of selectedGroups)
            for (const action of ["read", "execute"])
              if (
                !direct.some(
                  (share) =>
                    share.groupId === groupId && share.action === action,
                )
              )
                items.push({
                  key: `${member.id}:grant:${groupId}:${action}`,
                  commandId: "group.share.create",
                  input: {
                    workspaceId,
                    groupId,
                    memberId: member.id,
                    action,
                    effect: "allow",
                  },
                });
        }
      }
      const results: MemberResult[] = [];
      for (let offset = 0; offset < items.length; offset += 100) {
        const receipt = await api.managementBatch(
          { items: items.slice(offset, offset + 100) },
          `member-access-${crypto.randomUUID()}`,
        );
        for (const item of receipt.items)
          results.push({
            label: label(
              editing.find((member) => item.key.startsWith(`${member.id}:`)) ??
                editing[0],
            ),
            failed: item.status === "failed",
            message:
              item.status === "succeeded"
                ? t("success")
                : (item.error?.title ?? t("failed")),
          });
      }
      setOutcomes(
        results.length
          ? results
          : [{ label: "", failed: false, message: t("noChanges") }],
      );
      await reload();
      onDataChanged?.();
      if (results.every((item) => !item.failed)) setEditing([]);
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  async function executeMembership() {
    if (!operation) return;
    setPending(true);
    setError("");
    try {
      const receipt = await api.managementBatch(
        {
          items: chosen.map((member) => ({
            key: member.id,
            commandId:
              operation === "remove" ? "member.remove" : `member.${operation}`,
            input: { workspaceId, memberId: member.id },
          })),
        },
        `membership-${operation}-${crypto.randomUUID()}`,
      );
      setOutcomes(
        receipt.items.map((item) => ({
          label: label(
            chosen.find((member) => member.id === item.key) ?? chosen[0],
          ),
          failed: item.status === "failed",
          message:
            item.status === "succeeded"
              ? t("success")
              : (item.error?.title ?? t("failed")),
        })),
      );
      setOperation(undefined);
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  async function openDirectory() {
    setAdding(true);
    setAddSelected([]);
    setError("");
    try {
      const page = await api.listOrganizationMembers(organizationId);
      setDirectory(page.items);
    } catch (reason) {
      setError(accountError(reason));
    }
  }
  async function addFromDirectory() {
    setPending(true);
    setError("");
    try {
      const candidates = directory.filter((item) =>
        addSelected.includes(item.id),
      );
      const receipt = await api.managementBatch(
        {
          items: candidates.map((item) => ({
            key: item.id,
            commandId: "organization.member.workspace.assign",
            input: {
              organizationId,
              workspaceId,
              organizationMemberId: item.id,
              organizationMemberRevision: item.revision,
              role: addRole,
            },
          })),
        },
        `member-assign-${crypto.randomUUID()}`,
      );
      setOutcomes(
        receipt.items.map((item) => ({
          label:
            candidates.find((candidate) => candidate.id === item.key)
              ?.username || t("accounts"),
          failed: item.status === "failed",
          message:
            item.status === "succeeded"
              ? t("success")
              : (item.error?.title ?? t("failed")),
        })),
      );
      if (receipt.items.every((item) => item.status === "succeeded"))
        setAdding(false);
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  const inviteEmails = [
    ...new Set(
      inviteText
        .split(/[\n,;\t]/)
        .map((email) => email.trim().toLowerCase())
        .filter(Boolean),
    ),
  ];
  async function sendInvitations() {
    setPending(true);
    setError("");
    try {
      const expiresAt = Math.floor(Date.now() / 1000) + 7 * 24 * 60 * 60;
      const receipt = await api.managementBatch(
        {
          items: inviteEmails.map((email) => ({
            key: email,
            commandId: "member.invitation.create",
            input: { workspaceId, email, role: inviteRole, expiresAt },
          })),
        },
        `member-invite-${crypto.randomUUID()}`,
      );
      setInvitationResults(
        receipt.items.map((item) => ({
          label: item.key,
          failed: item.status === "failed",
          message:
            item.status === "succeeded"
              ? t("inviteNotEmail")
              : (item.error?.title ?? t("failed")),
          receipt:
            item.status === "succeeded"
              ? (item.data as MemberInvitationCreateReceipt)
              : undefined,
        })),
      );
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold">{t("members")}</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("membersHint")}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("refresh")}
            disabled={loading}
            onClick={() => void reload()}
          >
            <LuRefreshCw className={loading ? "animate-spin" : ""} />
          </Button>
          {mayManage ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="sm">
                  <LuPlus />
                  {t("addMember")}
                  <LuChevronDown />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void openDirectory()}>
                  <LuUsers />
                  {t("selectAccounts")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={() => {
                    setInviting(true);
                    setInvitationResults([]);
                    setInviteText("");
                  }}
                >
                  <LuMail />
                  {t("invite")}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => setProvisioning(true)}>
                  <LuUserPlus />
                  {t("createAccount")}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>
      </div>
      <AccountError message={error} onRetry={() => void reload()} />
      <div className="flex items-center gap-2">
        <div className="relative max-w-md flex-1">
          <LuSearch className="absolute left-3 top-2.5 size-4 text-muted-foreground" />
          <Input
            className="pl-9"
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setSelected([]);
            }}
            placeholder={t("search")}
          />
        </div>
        <Select
          value={roleFilter}
          onValueChange={(value) => {
            setRoleFilter(value);
            setSelected([]);
          }}
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("role")}</SelectItem>
            {(["owner", "admin", "lead", "operator", "member"] as const).map(
              (role) => (
                <SelectItem key={role} value={role}>
                  {t(role)}
                </SelectItem>
              ),
            )}
          </SelectContent>
        </Select>
      </div>
      {selected.length && mayManage ? (
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-border bg-muted/30 p-2">
          <span className="px-1 text-xs font-medium">
            {t("selected", { count: selected.length })}
          </span>
          <Button variant="outline" size="sm" onClick={() => openEdit(chosen)}>
            {t("editRoles")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => openEdit(chosen, true)}
          >
            {t("editGroups")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setOperation("suspend")}
          >
            {t("suspend")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setOperation("activate")}
          >
            {t("activate")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setOperation("remove")}
          >
            <LuTrash2 />
            {t("removeMember")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            onClick={() => setSelected([])}
          >
            <LuX />
            {t("clearSelection")}
          </Button>
        </div>
      ) : null}
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-border">
        <table className="w-full table-fixed">
          <thead className="sticky top-0 z-10 bg-background">
            <tr>
              <th className="w-10 px-3">
                <Checkbox
                  aria-label={t("selectAll")}
                  checked={
                    visible.length > 0 &&
                    visible.every((member) => selected.includes(member.id))
                  }
                  onCheckedChange={(checked) =>
                    setSelected(
                      checked ? visible.map((member) => member.id) : [],
                    )
                  }
                />
              </th>
              <th className={th}>{t("members")}</th>
              <th className={`${th} w-36`}>{t("role")}</th>
              <th className={`${th} hidden lg:table-cell`}>{t("groups")}</th>
              <th className={`${th} w-28`}>{t("membership")}</th>
              <th className={`${th} w-24 text-right`}>{t("actions")}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {loading
              ? Array.from({ length: 4 }, (_, index) => (
                  <tr key={index}>
                    <td colSpan={6} className="px-3 py-3">
                      <div className="h-5 animate-pulse rounded bg-muted" />
                    </td>
                  </tr>
                ))
              : visible.map((member) => (
                  <tr key={member.id} className="hover:bg-muted/30">
                    <td className="px-3">
                      <Checkbox
                        checked={selected.includes(member.id)}
                        aria-label={`${t("select")} ${label(member)}`}
                        onCheckedChange={(checked) =>
                          setSelected((items) =>
                            checked
                              ? [...items, member.id]
                              : items.filter((id) => id !== member.id),
                          )
                        }
                      />
                    </td>
                    <td className={td}>
                      <div className="truncate font-medium">
                        {label(member)}
                      </div>
                      <div className="truncate text-xs text-muted-foreground">
                        {member.identity?.email}
                      </div>
                    </td>
                    <td className={td}>
                      <Badge variant="secondary">{t(member.role)}</Badge>
                    </td>
                    <td
                      className={`${td} hidden truncate text-xs text-muted-foreground lg:table-cell`}
                    >
                      {effectiveGroups(member)}
                    </td>
                    <td className={td}>
                      <Badge variant="outline">{t(member.status)}</Badge>
                    </td>
                    <td className={`${td} text-right`}>
                      {mayManage ? (
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="size-8"
                              aria-label={t("actions")}
                            >
                              <LuEllipsis />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem
                              onSelect={() => openEdit([member])}
                            >
                              <LuPencil />
                              {t("editAccess")}
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onSelect={() => {
                                setSelected([member.id]);
                                setOperation(
                                  member.status === "active"
                                    ? "suspend"
                                    : "activate",
                                );
                              }}
                            >
                              <LuShield />
                              {t(
                                member.status === "active"
                                  ? "suspend"
                                  : "activate",
                              )}
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              className="text-destructive"
                              onSelect={() => {
                                setSelected([member.id]);
                                setOperation("remove");
                              }}
                            >
                              <LuTrash2 />
                              {t("removeMember")}
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      ) : null}
                    </td>
                  </tr>
                ))}
          </tbody>
        </table>
        {!loading && !visible.length ? (
          <div className="p-10 text-center text-sm text-muted-foreground">
            {t(query ? "noResults" : "noMembers")}
          </div>
        ) : null}
      </div>
      {outcomes.length ? (
        <div className="max-h-40 overflow-auto rounded-lg border border-border p-3">
          {outcomes.map((result, index) => (
            <div
              className={`flex justify-between gap-3 py-1 text-xs ${result.failed ? "text-destructive" : "text-muted-foreground"}`}
              key={`${result.label}-${index}`}
            >
              <span>{result.label}</span>
              <span>{result.message}</span>
            </div>
          ))}
        </div>
      ) : null}
      <AccountProvisioning
        open={provisioning}
        onOpenChange={setProvisioning}
        organizationId={organizationId}
        workspaceId={workspaceId}
        onCreated={() => {
          void reload();
          onDataChanged?.();
        }}
      />
      <Dialog
        open={editing.length > 0}
        onOpenChange={(open) => {
          if (!open && !pending) setEditing([]);
        }}
      >
        <DialogContent className="max-w-2xl" dismissible={!pending}>
          <DialogHeader>
            <DialogTitle>{t("editAccess")}</DialogTitle>
            <DialogDescription>
              {editing.length === 1
                ? label(editing[0])
                : t("selected", { count: editing.length })}
            </DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          <div
            className={`grid gap-5 ${groups.length ? "md:grid-cols-2" : "grid-cols-1"}`}
          >
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-sm font-medium">
                <Checkbox
                  id="member-edit-roles"
                  aria-label={t("editRoles")}
                  checked={changeRole}
                  onCheckedChange={(checked) => setChangeRole(checked === true)}
                />
                <Label htmlFor="member-edit-roles" className="cursor-pointer">
                  {t("editRoles")}
                </Label>
              </div>
              <RolePicker
                value={editRole}
                onChange={setEditRole}
                disabled={!changeRole}
                includeOwner={
                  editing.length === 1 && editing[0].role === "owner"
                }
              />
              <p className="text-xs leading-5 text-muted-foreground">
                {t(`${editRole}Help`)} {t("roleHint")}
              </p>
              <div className="max-h-40 overflow-auto rounded-md border border-border p-2 text-xs text-muted-foreground">
                {editing.map((member) => (
                  <div className="py-1" key={member.id}>
                    {label(member)} · {t(member.role)}
                    {changeRole ? ` → ${t(editRole)}` : ""}
                  </div>
                ))}
              </div>
            </div>
            {groups.length ? (
              <div className="space-y-3">
                <div className="flex items-center gap-2 text-sm font-medium">
                  <Checkbox
                    id="member-replace-groups"
                    aria-label={t("replaceGroups")}
                    checked={replaceGroups}
                    onCheckedChange={(checked) =>
                      setReplaceGroups(checked === true)
                    }
                  />
                  <Label
                    htmlFor="member-replace-groups"
                    className="cursor-pointer"
                  >
                    {t("replaceGroups")}
                  </Label>
                </div>
                <div className="max-h-64 space-y-1 overflow-auto rounded-md border border-border p-2">
                  {groups.map((group) => (
                    <div
                      className="flex items-center gap-2 rounded px-2 py-2 text-sm hover:bg-muted/30"
                      key={group.group.id}
                    >
                      <Checkbox
                        id={`member-group-${group.group.id}`}
                        aria-label={group.group.name}
                        disabled={!replaceGroups}
                        checked={selectedGroups.includes(group.group.id)}
                        onCheckedChange={(checked) =>
                          setSelectedGroups((items) =>
                            checked
                              ? [...items, group.group.id]
                              : items.filter((id) => id !== group.group.id),
                          )
                        }
                      />
                      <Label
                        htmlFor={`member-group-${group.group.id}`}
                        className="cursor-pointer"
                      >
                        {group.group.name}
                      </Label>
                    </div>
                  ))}
                </div>
                <p className="text-xs leading-5 text-muted-foreground">
                  {t("permissionChanges")}
                </p>
              </div>
            ) : null}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setEditing([])}
            >
              {t("cancel")}
            </Button>
            <Button
              disabled={
                pending ||
                (!changeRole && !(replaceGroups && groups.length > 0))
              }
              onClick={() => void editAccess()}
            >
              {pending ? t("processing") : t("save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(operation)}
        onOpenChange={(open) => {
          if (!open && !pending) setOperation(undefined);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t(
                operation === "remove"
                  ? "removeMember"
                  : operation === "suspend"
                    ? "suspend"
                    : "activate",
              )}
            </DialogTitle>
            <DialogDescription>
              {t("confirmChange")} · {t("selected", { count: chosen.length })}
            </DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          <ul className="max-h-48 overflow-auto text-sm">
            {chosen.map((member) => (
              <li key={member.id} className="py-1">
                {label(member)} · {t(member.role)} · {t(member.status)}
              </li>
            ))}
          </ul>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setOperation(undefined)}
            >
              {t("cancel")}
            </Button>
            <Button
              variant={operation === "remove" ? "destructive" : "default"}
              disabled={pending}
              onClick={() => void executeMembership()}
            >
              {pending ? t("processing") : t("confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={adding}
        onOpenChange={(open) => {
          if (!pending) setAdding(open);
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t("selectAccounts")}</DialogTitle>
            <DialogDescription>{t("scopeHint")}</DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          <Input
            placeholder={t("search")}
            value={addQuery}
            onChange={(event) => setAddQuery(event.target.value)}
          />
          <RolePicker value={addRole} onChange={setAddRole} />
          <div className="max-h-72 divide-y divide-border overflow-auto rounded-lg border border-border">
            {directory
              .filter(
                (entry) =>
                  !members.some(
                    (member) => member.accountId === entry.accountId,
                  ) &&
                  `${entry.username} ${entry.email}`
                    .toLowerCase()
                    .includes(addQuery.toLowerCase()),
              )
              .map((entry) => (
                <div
                  className="flex items-center gap-3 px-3 py-2.5"
                  key={entry.id}
                >
                  <Checkbox
                    id={`member-add-${entry.id}`}
                    aria-label={entry.username || entry.email}
                    checked={addSelected.includes(entry.id)}
                    onCheckedChange={(checked) =>
                      setAddSelected((items) =>
                        checked
                          ? [...items, entry.id]
                          : items.filter((id) => id !== entry.id),
                      )
                    }
                  />
                  <Label
                    htmlFor={`member-add-${entry.id}`}
                    className="min-w-0 cursor-pointer"
                  >
                    <span className="block truncate text-sm font-medium">
                      {entry.username || entry.email}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {entry.email}
                    </span>
                  </Label>
                </div>
              ))}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setAdding(false)}
            >
              {t("cancel")}
            </Button>
            <Button
              disabled={pending || !addSelected.length}
              onClick={() => void addFromDirectory()}
            >
              {pending
                ? t("processing")
                : `${t("assign")} (${addSelected.length})`}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={inviting}
        onOpenChange={(open) => {
          if (!pending) {
            setInviting(open);
            if (!open) setInvitationResults([]);
          }
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t("inviteMany")}</DialogTitle>
            <DialogDescription>{t("inviteExpires")}</DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          <Label>{t("emailList")}</Label>
          <textarea
            className="min-h-28 rounded-md border border-input bg-background p-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
            value={inviteText}
            onChange={(event) => setInviteText(event.target.value)}
            placeholder="name@company.com"
          />
          <RolePicker value={inviteRole} onChange={setInviteRole} />
          <div className="max-h-64 space-y-2 overflow-auto">
            {invitationResults.map((result) => (
              <div
                className="rounded-md border border-border p-3 text-xs"
                key={result.label}
              >
                <div className="font-medium">{result.label}</div>
                <p
                  className={`mt-1 ${result.failed ? "text-destructive" : "text-muted-foreground"}`}
                >
                  {result.message}
                </p>
                {result.receipt?.code ? (
                  <div className="mt-2 flex items-center gap-2">
                    <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1">
                      {result.receipt.code}
                    </code>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={async () => {
                        try {
                          await navigator.clipboard.writeText(
                            result.receipt?.code ?? "",
                          );
                          showSuccessToast(t("copied"));
                        } catch (reason) {
                          setError(accountError(reason));
                        }
                      }}
                    >
                      <LuCopy />
                      {t("copy")}
                    </Button>
                  </div>
                ) : null}
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setInviting(false)}
            >
              {t("close")}
            </Button>
            <Button
              disabled={
                pending ||
                !inviteEmails.length ||
                inviteEmails.length > 100 ||
                inviteEmails.some(
                  (email) => !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email),
                ) ||
                invitationResults.length > 0
              }
              onClick={() => void sendInvitations()}
            >
              {pending
                ? t("processing")
                : t("sendInvites", { count: inviteEmails.length })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuChevronLeft,
  LuChevronRight,
  LuFolder,
  LuListFilter,
  LuLoaderCircle,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuShield,
  LuTrash2,
  LuUsers,
  LuX,
} from "react-icons/lu";
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
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { createTauriDesktopApi } from "../../../apps/desktop/src/bridge";
import type { ResourceGrant } from "../../../apps/desktop/src/contracts";
import type {
  GroupRecord,
  GroupShareGrant,
  Member,
  ProfileRecord,
} from "../../../frontend/src/generated/command-registry";
import {
  changeGroupAssignment,
  type GroupAssignmentMode,
} from "./resource-input";
import "./resource-i18n";

const api = createTauriDesktopApi();
type Command = {
  key: string;
  commandId: string;
  input: Record<string, unknown>;
};
type OperationResult = {
  key: string;
  status: string;
  data?: unknown;
  error?: unknown;
};
type Action = "create" | "edit" | "assign" | "grant" | "delete" | "access";
type Pending = { action: Action; ids: string[] };
type GroupPermission = GroupShareGrant["action"];

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const detail = error as {
      detail?: string;
      message?: string;
      title?: string;
      code?: string;
    };
    return (
      detail.detail ||
      detail.message ||
      detail.title ||
      detail.code ||
      JSON.stringify(error)
    );
  }
  return String(error);
}
function memberLabel(member: Member | undefined, fallback: string): string {
  return member?.identity?.username || member?.identity?.email || fallback;
}

export function ManagementResources({
  organizationName,
  workspaceName,
  workspaceId,
  kind,
  initialGroupId,
}: {
  organizationName: string;
  workspaceName: string;
  workspaceId: string;
  kind: "groups" | "profiles";
  initialGroupId?: string;
}) {
  const { t, i18n } = useTranslation("managementResources");
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const [profiles, setProfiles] = useState<ProfileRecord[]>([]);
  const [members, setMembers] = useState<Member[]>([]);
  const [shares, setShares] = useState<GroupShareGrant[]>([]);
  const [query, setQuery] = useState("");
  const [groupFilter, setGroupFilter] = useState(initialGroupId || "all");
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [accessWarning, setAccessWarning] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [page, setPage] = useState(0);
  const [pending, setPending] = useState<Pending>();
  const [detailGroup, setDetailGroup] = useState<string>();
  const [detailTab, setDetailTab] = useState<"members" | "profiles">("members");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [step, setStep] = useState<"edit" | "review" | "result">("edit");
  const [results, setResults] = useState<OperationResult[]>([]);
  const [commands, setCommands] = useState<Command[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});
  const [createName, setCreateName] = useState("");
  const [prefix, setPrefix] = useState("");
  const [suffix, setSuffix] = useState("");
  const [note, setNote] = useState("");
  const [tags, setTags] = useState("");
  const [changeName, setChangeName] = useState(true);
  const [changeNote, setChangeNote] = useState(false);
  const [changeTags, setChangeTags] = useState(false);
  const [assignment, setAssignment] = useState<GroupAssignmentMode>("add");
  const [chosenGroups, setChosenGroups] = useState<string[]>([]);
  const [chosenMembers, setChosenMembers] = useState<string[]>([]);
  const [permission, setPermission] = useState<GroupPermission>("read");
  const [effect, setEffect] = useState<"allow" | "deny">("allow");
  const [directGrants, setDirectGrants] = useState<ResourceGrant[]>([]);
  const [resourceIds, setResourceIds] = useState<Record<string, string>>({});
  const [revokeIds, setRevokeIds] = useState<string[]>([]);
  const requestVersion = useRef(0);
  const operationKey = useRef("");
  const commandLabels = useRef<Record<string, string>>({});

  const load = useCallback(async () => {
    const version = ++requestVersion.current;
    setLoading(true);
    setLoadError("");
    setAccessWarning("");
    try {
      const [groupResult, profileResult, memberResult] =
        await Promise.allSettled([
          api.listGroups(workspaceId),
          (async () => {
            const records: ProfileRecord[] = [];
            const seen = new Set<string>();
            let cursor: string | undefined;
            do {
              const response = await api.listProfiles(false, {
                workspaceId,
                limit: 100,
                ...(cursor ? { cursor } : {}),
              });
              records.push(...response.items);
              cursor = response.nextCursor ?? undefined;
              if (cursor && seen.has(cursor))
                throw new Error("Profile pagination cursor repeated");
              if (cursor) seen.add(cursor);
            } while (cursor);
            return records;
          })(),
          api.listMembers(workspaceId),
        ]);
      if (version !== requestVersion.current) return;
      const warnings: string[] = [];
      if (groupResult.status === "fulfilled") setGroups(groupResult.value);
      else if (kind === "groups") throw groupResult.reason;
      else warnings.push(errorMessage(groupResult.reason));
      if (profileResult.status === "fulfilled")
        setProfiles(profileResult.value);
      else if (kind === "profiles") throw profileResult.reason;
      else warnings.push(errorMessage(profileResult.reason));
      if (memberResult.status === "fulfilled")
        setMembers(memberResult.value.items);
      else warnings.push(errorMessage(memberResult.reason));
      if (groupResult.status === "fulfilled") {
        const grants: PromiseSettledResult<GroupShareGrant[]>[] = [];
        for (let offset = 0; offset < groupResult.value.length; offset += 8) {
          const batch = groupResult.value.slice(offset, offset + 8);
          grants.push(
            ...(await Promise.allSettled(
              batch.map((record) =>
                api.listGroupShares({ workspaceId, groupId: record.group.id }),
              ),
            )),
          );
        }
        if (version !== requestVersion.current) return;
        const failedGroupIds = new Set(
          grants.flatMap((result, index) =>
            result.status === "rejected"
              ? [groupResult.value[index].group.id]
              : [],
          ),
        );
        const failedGroupNames = grants.flatMap((result, index) =>
          result.status === "rejected"
            ? [groupResult.value[index].group.name]
            : [],
        );
        setShares((previous) => [
          ...previous.filter((grant) => failedGroupIds.has(grant.groupId)),
          ...grants.flatMap((result) =>
            result.status === "fulfilled" ? result.value : [],
          ),
        ]);
        if (failedGroupNames.length)
          warnings.push(
            t("permissionErrorGroups", {
              groups: failedGroupNames.join(", "),
            }),
          );
      }
      setAccessWarning(warnings.join(" "));
    } catch (reason) {
      if (version === requestVersion.current)
        setLoadError(errorMessage(reason));
    } finally {
      if (version === requestVersion.current) setLoading(false);
    }
  }, [workspaceId, kind, t]);

  useEffect(() => {
    setSelected([]);
    setPage(0);
    setQuery("");
    setGroupFilter(initialGroupId || "all");
    setDetailGroup(undefined);
    setPending(undefined);
    void load();
    return () => {
      requestVersion.current += 1;
    };
  }, [load, initialGroupId]);
  const filtered = useMemo(() => {
    const text = query.trim().toLocaleLowerCase();
    if (kind === "groups")
      return groups
        .filter((record) =>
          record.group.name.toLocaleLowerCase().includes(text),
        )
        .map((record) => ({
          id: record.group.id,
          name: record.group.name,
          updated: record.group.updatedAt,
        }));
    return profiles
      .filter(
        (profile) =>
          `${profile.name} ${profile.note || ""} ${profile.tags.join(" ")}`
            .toLocaleLowerCase()
            .includes(text) &&
          (groupFilter === "all" ||
            (groupFilter === "ungrouped" && profile.groupIds.length === 0) ||
            profile.groupIds.includes(groupFilter)),
      )
      .map((profile) => ({
        id: profile.id,
        name: profile.name,
        updated: profile.updatedAt,
      }));
  }, [kind, groups, profiles, query, groupFilter]);
  const visible = filtered.slice(page * 25, (page + 1) * 25);
  const pages = Math.max(1, Math.ceil(filtered.length / 25));
  const assignmentPreview = useMemo(() => {
    if (kind !== "profiles" || pending?.action !== "assign") return [];
    return pending.ids.flatMap((id) => {
      const profile = profiles.find((value) => value.id === id);
      if (!profile) return [];
      const nextGroupIds = changeGroupAssignment(
        profile.groupIds,
        chosenGroups,
        assignment,
      );
      const changed =
        JSON.stringify([...nextGroupIds].sort()) !==
        JSON.stringify([...profile.groupIds].sort());
      const namesForGroups = (ids: string[]) =>
        ids.map(
          (groupId) =>
            groups.find((record) => record.group.id === groupId)?.group.name ??
            t("unavailableGroupName"),
        );
      return [
        {
          id: profile.id,
          name: profile.name,
          current: namesForGroups(profile.groupIds),
          next: namesForGroups(nextGroupIds),
          changed,
        },
      ];
    });
  }, [assignment, chosenGroups, groups, kind, pending, profiles, t]);
  const toggle = (
    id: string,
    chosen: string[],
    update: (value: string[]) => void,
  ) =>
    update(
      chosen.includes(id)
        ? chosen.filter((value) => value !== id)
        : [...chosen, id],
    );
  const nameFor = (id: string) =>
    kind === "groups"
      ? groups.find((record) => record.group.id === id)?.group.name ||
        t("groups")
      : profiles.find((profile) => profile.id === id)?.name || t("profiles");

  async function open(action: Action, ids: string[]) {
    setPending({ action, ids });
    setStep("edit");
    setError("");
    setResults([]);
    setCommands([]);
    setRevokeIds([]);
    setPrefix("");
    setSuffix("");
    setCreateName("");
    setAssignment("add");
    setChosenGroups([]);
    setChosenMembers([]);
    setEffect("allow");
    setPermission("read");
    setDirectGrants([]);
    setResourceIds({});
    setNames(Object.fromEntries(ids.map((id) => [id, nameFor(id)])));
    const single =
      ids.length === 1 && kind === "profiles"
        ? profiles.find((profile) => profile.id === ids[0])
        : undefined;
    setNote(single?.note ?? "");
    setTags(single?.tags.join(", ") ?? "");
    setChangeName(true);
    setChangeNote(Boolean(single));
    setChangeTags(Boolean(single));
    if ((action === "access" || action === "grant") && kind === "profiles") {
      setBusy(true);
      try {
        const bindings = await Promise.all(
          ids.map((id) => api.getProfileBinding(id)),
        );
        const map = Object.fromEntries(
          bindings.map((binding) => [binding.profileId, binding.resourceId]),
        );
        setResourceIds(map);
        const grants = await Promise.all(
          bindings.map((binding) =>
            api.listGrants(workspaceId, binding.resourceId),
          ),
        );
        setDirectGrants(grants.flatMap((item) => item.items));
      } catch (reason) {
        setError(errorMessage(reason));
      } finally {
        setBusy(false);
      }
    }
  }

  function buildCommands(): Command[] {
    if (!pending) return [];
    const output: Command[] = [];
    const labels: Record<string, string> = {};
    function add(
      commandId: string,
      key: string,
      input: Record<string, unknown>,
      label: string,
    ) {
      output.push({ commandId, key, input });
      labels[key] = label;
    }
    if (pending.action === "create") {
      const name = createName.trim();
      if (!name) throw new Error(t("invalidName"));
      if (name.length > 120) throw new Error(t("groupNameTooLong"));
      add(
        "group.create",
        "create",
        {
          workspaceId,
          groupId: `grp_${crypto.randomUUID().replaceAll("-", "")}`,
          name,
          appearance: null,
        },
        name,
      );
    } else if (pending.action === "edit") {
      for (const id of pending.ids) {
        if (changeName && !names[id]?.trim()) throw new Error(t("missingName"));
        if (kind === "groups") {
          const record = groups.find((value) => value.group.id === id);
          if (record && record.group.name !== names[id].trim())
            add(
              "group.update",
              id,
              {
                workspaceId,
                groupId: id,
                expectedRevision: record.revision,
                name: names[id].trim(),
                appearance: record.group.appearance,
              },
              names[id],
            );
        } else {
          const profile = profiles.find((value) => value.id === id);
          if (!profile) continue;
          const nextName = changeName ? names[id].trim() : profile.name;
          const nextNote = changeNote ? note.trim() || null : profile.note;
          const nextTags = changeTags
            ? [
                ...new Set(
                  tags
                    .split(",")
                    .map((value) => value.trim())
                    .filter(Boolean),
                ),
              ]
            : profile.tags;
          if (
            nextName !== profile.name ||
            nextNote !== profile.note ||
            JSON.stringify(nextTags) !== JSON.stringify(profile.tags)
          )
            add(
              "profile.update",
              id,
              {
                workspaceId,
                profileId: id,
                expectedRevision: profile.revision,
                name: nextName,
                note: nextNote,
                tags: nextTags,
              },
              nextName,
            );
        }
      }
    } else if (pending.action === "assign") {
      for (const id of pending.ids) {
        const profile = profiles.find((value) => value.id === id);
        if (!profile) continue;
        const nextGroups = changeGroupAssignment(
          profile.groupIds,
          chosenGroups,
          assignment,
        );
        if (
          JSON.stringify([...nextGroups].sort()) !==
          JSON.stringify([...profile.groupIds].sort())
        )
          add(
            "profile.groups.replace",
            id,
            {
              workspaceId,
              profileId: id,
              expectedRevision: profile.revision,
              groupIds: nextGroups,
            },
            profile.name,
          );
      }
    } else if (pending.action === "delete") {
      for (const id of pending.ids) {
        if (kind === "groups") {
          const group = groups.find((record) => record.group.id === id);
          if (
            profiles.some((profile) => profile.groupIds.includes(id)) ||
            shares.some((grant) => grant.groupId === id && !grant.revokedAt)
          )
            throw new Error(`${nameFor(id)}: ${t("groupDeleteBlocked")}`);
          if (group)
            add(
              "group.delete",
              id,
              { workspaceId, groupId: id, expectedRevision: group.revision },
              group.group.name,
            );
        } else {
          const profile = profiles.find((value) => value.id === id);
          if (profile)
            add(
              "profile.delete",
              id,
              {
                profileId: id,
                revision: profile.revision,
                confirmation: "QUARANTINE_PROFILE_DATA",
              },
              profile.name,
            );
        }
      }
    } else if (pending.action === "grant" || pending.action === "access") {
      if (revokeIds.length) {
        if (kind === "groups")
          for (const grant of shares.filter((value) =>
            revokeIds.includes(value.id),
          ))
            add(
              "group.share.revoke",
              grant.id,
              {
                workspaceId,
                groupId: grant.groupId,
                grantId: grant.id,
                expectedRevision: grant.revision,
              },
              memberLabel(
                members.find((member) => member.id === grant.memberId),
                t("unknownIdentity"),
              ),
            );
        else
          for (const grant of directGrants.filter((value) =>
            revokeIds.includes(value.id),
          ))
            add(
              "grant.revoke",
              grant.id,
              { grantId: grant.id },
              memberLabel(
                members.find((member) => member.id === grant.memberId),
                t("unknownIdentity"),
              ),
            );
      } else {
        if (!chosenMembers.length) throw new Error(t("requiresMember"));
        for (const id of pending.ids)
          for (const memberId of chosenMembers) {
            if (kind === "groups")
              add(
                "group.share.create",
                `${id}:${memberId}:${permission}`,
                {
                  workspaceId,
                  groupId: id,
                  memberId,
                  action: permission,
                  effect,
                },
                `${nameFor(id)} · ${memberLabel(
                  members.find((member) => member.id === memberId),
                  t("unknownIdentity"),
                )}`,
              );
            else {
              const resourceId = resourceIds[id];
              if (!resourceId) throw new Error(t("profileIdUnknown"));
              if (permission === "share")
                throw new Error(t("noGrantSelection"));
              add(
                "grant.create",
                `${id}:${memberId}:${permission}`,
                {
                  id: `grant_${crypto.randomUUID().replaceAll("-", "")}`,
                  workspaceId,
                  resourceId,
                  memberId,
                  action: permission,
                  effect,
                },
                `${nameFor(id)} · ${memberLabel(
                  members.find((member) => member.id === memberId),
                  t("unknownIdentity"),
                )}`,
              );
            }
          }
      }
    }
    if (output.length > 100) throw new Error(t("limit"));
    commandLabels.current = labels;
    return output;
  }
  function review() {
    setError("");
    try {
      const next = buildCommands();
      if (!next.length) throw new Error(t("noChanges"));
      setCommands(next);
      operationKey.current = crypto.randomUUID();
      if (pending?.action === "create") {
        void execute(next);
        return;
      }
      setStep("review");
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }
  async function execute(items: Command[] = commands) {
    setBusy(true);
    setError("");
    try {
      const response = await api.managementBatch({
        items,
        idempotencyKey: operationKey.current,
      });
      setResults((current) => [
        ...current.filter(
          (result) => !items.some((item) => item.key === result.key),
        ),
        ...response.items,
      ]);
      setStep("result");
      setSelected([]);
      await load();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  const groupMembers = (groupId: string) => [
    ...new Set(
      shares
        .filter(
          (grant) =>
            grant.groupId === groupId &&
            !grant.revokedAt &&
            grant.effect === "allow",
        )
        .map((grant) =>
          memberLabel(
            members.find((member) => member.id === grant.memberId),
            t("unknownIdentity"),
          ),
        ),
    ),
  ];
  const targetShares = shares.filter(
    (grant) => pending?.ids.includes(grant.groupId) && !grant.revokedAt,
  );
  const displayedGrants = kind === "groups" ? targetShares : directGrants;
  const title =
    pending?.action === "create"
      ? t("create")
      : pending?.action === "edit"
        ? t(kind === "profiles" ? "editMetadata" : "editNames")
        : pending?.action === "assign"
          ? t("assign")
          : pending?.action === "delete"
            ? t("confirmDelete")
            : t("permissions");

  return (
    <section
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-4"
      aria-label={t(kind)}
    >
      <div className="flex shrink-0 items-center justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-base font-semibold">{t(kind)}</h1>
          <p className="mt-1 text-xs text-muted-foreground">
            {t(
              kind === "groups"
                ? "managementDescription"
                : "profileDescription",
            )}
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={loading}
            onClick={() => void load()}
            aria-label={t("refresh")}
          >
            <LuRefreshCw
              className={loading ? "size-4 animate-spin" : "size-4"}
            />
          </Button>
          {kind === "groups" && (
            <Button size="sm" onClick={() => void open("create", [])}>
              <LuPlus className="size-4" />
              {t("createOne")}
            </Button>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <div className="relative min-w-0 flex-1 sm:max-w-sm">
          <LuSearch className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
          <Input
            aria-label={t("search")}
            value={query}
            placeholder={t("search")}
            onChange={(event) => {
              setQuery(event.target.value);
              setPage(0);
              setSelected([]);
            }}
            className="h-9 pl-9"
          />
        </div>
        {kind === "profiles" && (
          <Select
            value={groupFilter}
            onValueChange={(value) => {
              setGroupFilter(value);
              setPage(0);
              setSelected([]);
            }}
          >
            <SelectTrigger className="h-9 w-44" aria-label={t("filterGroup")}>
              <LuListFilter className="mr-1 size-4" />
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("filterGroup")}</SelectItem>
              <SelectItem value="ungrouped">{t("ungrouped")}</SelectItem>
              {groups.map((record) => (
                <SelectItem key={record.group.id} value={record.group.id}>
                  {record.group.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <span className="hidden text-xs text-muted-foreground sm:inline">
          {t("rows", { count: filtered.length })}
        </span>
      </div>
      {loadError && (
        <div
          role="alert"
          className="flex shrink-0 items-center justify-between gap-2 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
        >
          <span>{loadError}</span>
          <Button size="sm" variant="outline" onClick={() => void load()}>
            {t("retry")}
          </Button>
        </div>
      )}
      {accessWarning && (
        <p role="status" className="text-xs text-warning-text">
          {accessWarning}
        </p>
      )}
      <ScrollArea className="min-h-0 flex-1 rounded-md border border-border">
        <table className="w-full table-fixed text-left text-sm">
          <thead className="sticky top-0 z-10 bg-muted">
            <tr>
              <th className="w-10 p-3">
                <Checkbox
                  aria-label={t("chooseAll")}
                  checked={
                    visible.length > 0 &&
                    visible.every((item) => selected.includes(item.id))
                  }
                  onCheckedChange={(checked) =>
                    setSelected(checked ? visible.map((item) => item.id) : [])
                  }
                />
              </th>
              <th className="p-3 text-xs font-medium text-muted-foreground">
                {t("name")}
              </th>
              <th className="hidden w-48 p-3 text-xs font-medium text-muted-foreground md:table-cell">
                {t(kind === "groups" ? "members" : "selectedGroups")}
              </th>
              <th className="hidden w-28 p-3 text-xs font-medium text-muted-foreground lg:table-cell">
                {t("updated")}
              </th>
              <th className="w-36 p-3 text-right text-xs font-medium text-muted-foreground">
                {t("actions")}
              </th>
            </tr>
          </thead>
          <tbody>
            {loading && !filtered.length
              ? Array.from({ length: 5 }, (_, index) => (
                  <tr key={index}>
                    <td colSpan={5} className="p-3">
                      <div className="h-7 animate-pulse rounded bg-muted" />
                    </td>
                  </tr>
                ))
              : visible.map((item) => (
                  <tr
                    key={item.id}
                    className="border-t border-border hover:bg-muted/50"
                  >
                    <td className="p-3">
                      <Checkbox
                        aria-label={t("selectResource", { name: item.name })}
                        checked={selected.includes(item.id)}
                        onCheckedChange={() =>
                          toggle(item.id, selected, setSelected)
                        }
                      />
                    </td>
                    <td className="min-w-0 p-3">
                      <button
                        type="button"
                        className="block max-w-full truncate text-left font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        title={item.name}
                        onClick={() => {
                          if (kind === "groups") {
                            setDetailGroup(item.id);
                            setDetailTab("members");
                            return;
                          }
                          void open("edit", [item.id]);
                        }}
                      >
                        {item.name}
                      </button>
                      {kind === "groups" && (
                        <span className="text-xs text-muted-foreground">
                          {t("rows", {
                            count: profiles.filter((profile) =>
                              profile.groupIds.includes(item.id),
                            ).length,
                          })}
                        </span>
                      )}
                    </td>
                    <td
                      className="hidden truncate p-3 text-xs text-muted-foreground md:table-cell"
                      title={
                        kind === "groups"
                          ? groupMembers(item.id).join(", ")
                          : profiles
                              .find((value) => value.id === item.id)
                              ?.groupIds.map(
                                (id) =>
                                  groups.find(
                                    (record) => record.group.id === id,
                                  )?.group.name,
                              )
                              .filter(Boolean)
                              .join(", ")
                      }
                    >
                      {kind === "groups"
                        ? groupMembers(item.id).join(", ") || t("noGrants")
                        : profiles
                            .find((value) => value.id === item.id)
                            ?.groupIds.map(
                              (id) =>
                                groups.find((record) => record.group.id === id)
                                  ?.group.name,
                            )
                            .filter(Boolean)
                            .join(", ") || t("ungrouped")}
                    </td>
                    <td className="hidden p-3 text-xs text-muted-foreground lg:table-cell">
                      {new Date(item.updated).toLocaleDateString(i18n.language)}
                    </td>
                    <td className="p-2">
                      <div className="flex items-center justify-end gap-1 whitespace-nowrap">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-8 px-2 text-xs"
                          onClick={() =>
                            kind === "groups"
                              ? setDetailGroup(item.id)
                              : void open("access", [item.id])
                          }
                        >
                          {t(kind === "groups" ? "modify" : "permissions")}
                        </Button>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="size-8"
                              aria-label={t("actionsFor", { name: item.name })}
                            >
                              <span aria-hidden="true">···</span>
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent
                            align="end"
                            className="w-52 text-xs"
                          >
                            <DropdownMenuItem
                              onSelect={() => void open("edit", [item.id])}
                            >
                              <LuPencil className="size-4" />
                              {t("edit")}
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onSelect={() => void open("access", [item.id])}
                            >
                              <LuShield className="size-4" />
                              {t("permissions")}
                            </DropdownMenuItem>
                            {kind === "profiles" && (
                              <DropdownMenuItem
                                onSelect={() => void open("assign", [item.id])}
                              >
                                <LuFolder className="size-4" />
                                {t("assign")}
                              </DropdownMenuItem>
                            )}
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              variant="destructive"
                              onSelect={() => void open("delete", [item.id])}
                            >
                              <LuTrash2 className="size-4" />
                              {t("delete")}
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </div>
                    </td>
                  </tr>
                ))}
          </tbody>
        </table>
        {!loading && !filtered.length && !loadError && (
          <div className="grid justify-items-center gap-2 py-12 text-center">
            <LuFolder className="size-6 text-muted-foreground" />
            <h2 className="text-sm font-medium">{t("empty")}</h2>
            <p className="max-w-sm text-xs text-muted-foreground">
              {t("emptyHint")}
            </p>
            {kind === "groups" && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void open("create", [])}
              >
                {t("createOne")}
              </Button>
            )}
          </div>
        )}
      </ScrollArea>
      <div className="flex shrink-0 items-center justify-between text-xs text-muted-foreground">
        <span>{t("rows", { count: filtered.length })}</span>
        <div className="flex items-center gap-2">
          <Button
            size="icon"
            variant="ghost"
            className="size-7"
            disabled={page === 0}
            aria-label={t("back")}
            onClick={() => setPage((value) => value - 1)}
          >
            <LuChevronLeft className="size-4" />
          </Button>
          {page + 1} / {pages}
          <Button
            size="icon"
            variant="ghost"
            className="size-7"
            disabled={page + 1 >= pages}
            aria-label={t("next")}
            onClick={() => setPage((value) => value + 1)}
          >
            <LuChevronRight className="size-4" />
          </Button>
        </div>
      </div>
      {selected.length > 0 && (
        <div className="flex shrink-0 flex-wrap items-center gap-1 rounded-md border border-border bg-muted px-2 py-1.5">
          <span className="mr-2 text-xs font-medium">
            {t("selected", { count: selected.length })}
          </span>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void open("edit", selected)}
          >
            {t("editNames")}
          </Button>
          {kind === "profiles" && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void open("assign", selected)}
            >
              {t("assign")}
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void open("grant", selected)}
          >
            {t("grant")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void open("access", selected)}
          >
            {t("revoke")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="text-destructive"
            onClick={() => void open("delete", selected)}
          >
            {t("delete")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto"
            onClick={() => setSelected([])}
          >
            <LuX className="size-3.5" />
            {t("clear")}
          </Button>
        </div>
      )}

      <Dialog
        open={Boolean(pending)}
        onOpenChange={(value) => {
          if (!value && !busy) {
            setPending(undefined);
            setCommands([]);
          }
        }}
      >
        <DialogContent className="flex max-h-5/6 w-full max-w-3xl flex-col overflow-hidden">
          <DialogHeader>
            <DialogTitle>
              {step === "result"
                ? t("result")
                : pending?.action === "create"
                  ? t("createOne")
                  : title}
            </DialogTitle>
            <DialogDescription className="space-y-1">
              <span className="block">
                {organizationName} / {workspaceName}
                {pending?.ids.length
                  ? ` · ${t("selected", { count: pending.ids.length })}`
                  : ""}
              </span>
              <span className="block">{t("scopeWarning")}</span>
            </DialogDescription>
          </DialogHeader>
          <ScrollArea className="min-h-0 flex-1">
            <div className="space-y-4 pr-3">
              {error && (
                <p
                  role="alert"
                  className="rounded border border-destructive/30 p-3 text-sm text-destructive"
                >
                  {error}
                </p>
              )}
              {step === "edit" && pending?.action === "create" && (
                <div className="space-y-2">
                  <Label htmlFor="management-group-name">{t("name")}</Label>
                  <Input
                    id="management-group-name"
                    value={createName}
                    onChange={(event) => setCreateName(event.target.value)}
                    maxLength={120}
                    autoFocus
                    disabled={busy}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" && !busy) review();
                    }}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("createHint")}
                  </p>
                </div>
              )}
              {step === "edit" && pending?.action === "edit" && (
                <>
                  {pending.ids.length > 1 && (
                    <div className="flex flex-wrap items-center gap-2">
                      <Input
                        aria-label={t("prefix")}
                        placeholder={t("prefix")}
                        value={prefix}
                        className="w-36"
                        onChange={(event) => setPrefix(event.target.value)}
                      />
                      <Input
                        aria-label={t("suffix")}
                        placeholder={t("suffix")}
                        value={suffix}
                        className="w-36"
                        onChange={(event) => setSuffix(event.target.value)}
                      />
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() =>
                          setNames(
                            Object.fromEntries(
                              pending.ids.map((id) => [
                                id,
                                `${prefix}${nameFor(id)}${suffix}`,
                              ]),
                            ),
                          )
                        }
                      >
                        {t("applyNames")}
                      </Button>
                    </div>
                  )}
                  {kind === "profiles" && (
                    <div className="flex items-center gap-2 text-sm">
                      <Checkbox
                        id="resource-change-name"
                        aria-label={t("changeName")}
                        checked={changeName}
                        onCheckedChange={(value) =>
                          setChangeName(value === true)
                        }
                      />
                      <Label
                        htmlFor="resource-change-name"
                        className="cursor-pointer"
                      >
                        {t("changeName")}
                      </Label>
                    </div>
                  )}
                  {(kind === "groups" || changeName) && (
                    <div className="overflow-hidden rounded-md border border-border">
                      <table className="w-full table-fixed text-left text-sm">
                        <thead className="bg-muted text-xs text-muted-foreground">
                          <tr>
                            <th className="p-2">{t("old")}</th>
                            <th className="p-2">{t("new")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {pending.ids.map((id) => (
                            <tr key={id} className="border-t border-border">
                              <td className="truncate p-2" title={nameFor(id)}>
                                {nameFor(id)}
                              </td>
                              <td className="p-2">
                                <Input
                                  aria-label={`${t("new")} ${nameFor(id)}`}
                                  value={names[id] || ""}
                                  onChange={(event) =>
                                    setNames((current) => ({
                                      ...current,
                                      [id]: event.target.value,
                                    }))
                                  }
                                />
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                  {kind === "profiles" && (
                    <>
                      <p className="text-xs text-muted-foreground">
                        {t("metadataHelp")}
                      </p>
                      <div className="flex items-center gap-2 text-sm">
                        <Checkbox
                          id="resource-change-note"
                          aria-label={t("changeNote")}
                          checked={changeNote}
                          onCheckedChange={(value) =>
                            setChangeNote(value === true)
                          }
                        />
                        <Label
                          htmlFor="resource-change-note"
                          className="cursor-pointer"
                        >
                          {t("changeNote")}
                        </Label>
                      </div>
                      {changeNote && (
                        <Textarea
                          aria-label={t("note")}
                          value={note}
                          onChange={(event) => setNote(event.target.value)}
                          rows={3}
                        />
                      )}
                      <div className="flex items-center gap-2 text-sm">
                        <Checkbox
                          id="resource-change-tags"
                          aria-label={t("changeTags")}
                          checked={changeTags}
                          onCheckedChange={(value) =>
                            setChangeTags(value === true)
                          }
                        />
                        <Label
                          htmlFor="resource-change-tags"
                          className="cursor-pointer"
                        >
                          {t("changeTags")}
                        </Label>
                      </div>
                      {changeTags && (
                        <Input
                          aria-label={t("tags")}
                          value={tags}
                          onChange={(event) => setTags(event.target.value)}
                        />
                      )}
                    </>
                  )}
                </>
              )}
              {step === "edit" && pending?.action === "assign" && (
                <>
                  <Label>{t("assignment")}</Label>
                  <Select
                    value={assignment}
                    onValueChange={(value) =>
                      setAssignment(value as GroupAssignmentMode)
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {(["add", "remove", "replace"] as const).map((mode) => (
                        <SelectItem key={mode} value={mode}>
                          {t(mode === "add" ? "keep" : mode)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <div className="grid gap-2 rounded-md border border-border p-3">
                    {groups.map((record) => (
                      <div
                        key={record.group.id}
                        className="flex items-center gap-2 py-1 text-sm"
                      >
                        <Checkbox
                          id={`group-${record.group.id}`}
                          aria-label={record.group.name}
                          checked={chosenGroups.includes(record.group.id)}
                          onCheckedChange={() =>
                            toggle(
                              record.group.id,
                              chosenGroups,
                              setChosenGroups,
                            )
                          }
                        />
                        <Label
                          htmlFor={`group-${record.group.id}`}
                          className="cursor-pointer"
                        >
                          {record.group.name}
                        </Label>
                      </div>
                    ))}
                  </div>
                  <section
                    className="space-y-2 rounded-md border border-border bg-muted/20 p-3"
                    aria-live="polite"
                  >
                    <p className="text-sm font-medium">
                      {t("assignmentImpact", {
                        changed: assignmentPreview.filter(
                          (item) => item.changed,
                        ).length,
                        unchanged: assignmentPreview.filter(
                          (item) => !item.changed,
                        ).length,
                      })}
                    </p>
                    <ScrollArea className="max-h-40">
                      <ul className="space-y-2 pr-3">
                        {assignmentPreview.map((item) => (
                          <li
                            key={item.id}
                            className="grid gap-0.5 border-b border-border pb-2 text-xs last:border-0 last:pb-0"
                          >
                            <span className="font-medium">{item.name}</span>
                            <span className="text-muted-foreground">
                              {item.current.join(", ") || t("noAssignedGroups")}{" "}
                              → {item.next.join(", ") || t("noAssignedGroups")}
                              {!item.changed ? ` · ${t("unchanged")}` : ""}
                            </span>
                          </li>
                        ))}
                      </ul>
                    </ScrollArea>
                  </section>
                </>
              )}
              {step === "edit" && pending?.action === "delete" && (
                <>
                  <p className="text-sm">
                    {t("deleteText", { count: pending.ids.length })}
                  </p>
                  <p className="text-sm text-warning-text">
                    {t(
                      kind === "groups"
                        ? "deleteGroupWarning"
                        : "deleteProfileWarning",
                    )}
                  </p>
                  <ul className="space-y-1 text-sm">
                    {pending.ids.map((id) => (
                      <li key={id}>{nameFor(id)}</li>
                    ))}
                  </ul>
                </>
              )}
              {step === "edit" &&
                (pending?.action === "grant" ||
                  pending?.action === "access") && (
                  <>
                    <p className="text-xs text-muted-foreground">
                      {t("inherited")}
                    </p>
                    {displayedGrants.length > 0 && (
                      <div className="rounded-md border border-border">
                        <table className="w-full table-fixed text-left text-xs">
                          <thead className="bg-muted">
                            <tr>
                              <th className="w-9 p-2" />
                              <th className="p-2">{t("member")}</th>
                              <th className="w-28 p-2">{t("capability")}</th>
                              <th className="w-24 p-2">{t("effect")}</th>
                            </tr>
                          </thead>
                          <tbody>
                            {displayedGrants.map((grant) => (
                              <tr
                                key={grant.id}
                                className="border-t border-border"
                              >
                                <td className="p-2">
                                  <Checkbox
                                    aria-label={t("revoke")}
                                    checked={revokeIds.includes(grant.id)}
                                    onCheckedChange={() =>
                                      toggle(grant.id, revokeIds, setRevokeIds)
                                    }
                                  />
                                </td>
                                <td className="truncate p-2">
                                  {memberLabel(
                                    members.find(
                                      (member) => member.id === grant.memberId,
                                    ),
                                    t("unknownIdentity"),
                                  )}
                                </td>
                                <td className="p-2">{t(grant.action)}</td>
                                <td className="p-2">{t(grant.effect)}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </div>
                    )}
                    {revokeIds.length === 0 && (
                      <>
                        <p className="text-sm font-medium">{t("grant")}</p>
                        <div className="grid max-h-48 gap-1 overflow-auto rounded-md border border-border p-3">
                          {members
                            .filter((member) => member.status === "active")
                            .map((member) => (
                              <div
                                key={member.id}
                                className="flex items-center gap-2 py-1 text-sm"
                              >
                                <Checkbox
                                  id={`member-${member.id}`}
                                  aria-label={memberLabel(
                                    member,
                                    t("unknownIdentity"),
                                  )}
                                  checked={chosenMembers.includes(member.id)}
                                  onCheckedChange={() =>
                                    toggle(
                                      member.id,
                                      chosenMembers,
                                      setChosenMembers,
                                    )
                                  }
                                />
                                <Label
                                  htmlFor={`member-${member.id}`}
                                  className="truncate cursor-pointer"
                                >
                                  {memberLabel(member, t("unknownIdentity"))}
                                </Label>
                              </div>
                            ))}
                          {!members.length && (
                            <p className="text-xs text-muted-foreground">
                              {t("noMembers")}
                            </p>
                          )}
                        </div>
                        <div className="grid grid-cols-2 gap-3">
                          <div className="space-y-1">
                            <Label>{t("capability")}</Label>
                            <Select
                              value={permission}
                              onValueChange={(value) =>
                                setPermission(value as GroupPermission)
                              }
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
                                    ...(kind === "groups" ? ["share"] : []),
                                  ] as GroupPermission[]
                                ).map((value) => (
                                  <SelectItem key={value} value={value}>
                                    {t(value)}
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          </div>
                          <div className="space-y-1">
                            <Label>{t("effect")}</Label>
                            <Select
                              value={effect}
                              onValueChange={(value) =>
                                setEffect(value as "allow" | "deny")
                              }
                            >
                              <SelectTrigger>
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value="allow">
                                  {t("allow")}
                                </SelectItem>
                                <SelectItem value="deny">
                                  {t("deny")}
                                </SelectItem>
                              </SelectContent>
                            </Select>
                          </div>
                        </div>
                      </>
                    )}
                  </>
                )}
              {step === "review" && (
                <>
                  <p className="text-sm">
                    {t("summary", { count: commands.length })}
                  </p>
                  <div className="rounded-md border border-border">
                    {commands.map((command) => (
                      <div
                        key={command.key}
                        className="flex items-center justify-between gap-3 border-b border-border p-2 text-sm last:border-0"
                      >
                        <span className="truncate">
                          {commandLabels.current[command.key]}
                        </span>
                        <span className="shrink-0 text-xs text-muted-foreground">
                          {t("pending")}
                        </span>
                      </div>
                    ))}
                  </div>
                </>
              )}
              {step === "result" && (
                <>
                  <div className="flex items-center gap-4 text-sm">
                    <span className="text-success-text">
                      {t("successful", {
                        count: results.filter(
                          (result) => result.status === "succeeded",
                        ).length,
                      })}
                    </span>
                    <span className="text-destructive">
                      {t("failedCount", {
                        count: results.filter(
                          (result) => result.status === "failed",
                        ).length,
                      })}
                    </span>
                  </div>
                  <div className="rounded-md border border-border">
                    {results.map((result) => (
                      <div
                        key={result.key}
                        className="space-y-1 border-b border-border p-3 last:border-0"
                      >
                        <div className="flex items-center justify-between gap-2 text-sm">
                          <span className="truncate">
                            {commandLabels.current[result.key] ||
                              t("deleteName")}
                          </span>
                          <span
                            className={
                              result.status === "succeeded"
                                ? "text-success-text"
                                : "text-destructive"
                            }
                          >
                            {t(
                              result.status === "succeeded"
                                ? "success"
                                : "failure",
                            )}
                          </span>
                        </div>
                        {result.status === "failed" && (
                          <p className="text-xs text-destructive">
                            {errorMessage(result.error)}
                          </p>
                        )}
                      </div>
                    ))}
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("reviewFailed")}
                  </p>
                </>
              )}
            </div>
          </ScrollArea>
          <DialogFooter className="mt-3 shrink-0">
            <Button
              variant="outline"
              disabled={busy}
              onClick={() =>
                step === "review" ? setStep("edit") : setPending(undefined)
              }
            >
              {t(step === "review" ? "back" : "close")}
            </Button>
            {step === "edit" && (
              <Button
                disabled={
                  busy || (pending?.action === "create" && !createName.trim())
                }
                onClick={review}
              >
                {busy ? (
                  <LuLoaderCircle className="size-4 animate-spin" />
                ) : pending?.action === "create" ? (
                  <LuPlus className="size-4" />
                ) : null}
                {pending?.action === "create" ? t("createOne") : t("preview")}
              </Button>
            )}
            {step === "review" && (
              <Button
                disabled={busy}
                variant={
                  pending?.action === "delete" ? "destructive" : "default"
                }
                onClick={() => void execute()}
              >
                {busy ? (
                  <LuLoaderCircle className="size-4 animate-spin" />
                ) : (
                  <LuCheck className="size-4" />
                )}
                {t("apply")}
              </Button>
            )}
            {step === "result" &&
              results.some((result) => result.status === "failed") && (
                <Button
                  disabled={busy}
                  onClick={() => {
                    operationKey.current = crypto.randomUUID();
                    void execute(
                      commands.filter((command) =>
                        results.some(
                          (result) =>
                            result.key === command.key &&
                            result.status === "failed",
                        ),
                      ),
                    );
                  }}
                >
                  {t("retryFailed")}
                </Button>
              )}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(detailGroup)}
        onOpenChange={(value) => {
          if (!value) setDetailGroup(undefined);
        }}
      >
        <DialogContent className="flex max-h-5/6 max-w-3xl flex-col overflow-hidden">
          <DialogHeader>
            <DialogTitle>
              {groups.find((record) => record.group.id === detailGroup)?.group
                .name || t("groupDetail")}
            </DialogTitle>
            <DialogDescription>{t("inherited")}</DialogDescription>
          </DialogHeader>
          <div className="flex items-center gap-2 border-b border-border pb-2">
            <Button
              variant={detailTab === "members" ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setDetailTab("members")}
            >
              <LuUsers className="size-4" />
              {t("groupMembers")}
            </Button>
            <Button
              variant={detailTab === "profiles" ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setDetailTab("profiles")}
            >
              <LuFolder className="size-4" />
              {t("groupProfiles")}
            </Button>
          </div>
          <ScrollArea className="min-h-0 flex-1">
            <div className="space-y-2">
              {detailTab === "members"
                ? shares
                    .filter(
                      (grant) =>
                        grant.groupId === detailGroup && !grant.revokedAt,
                    )
                    .map((grant) => (
                      <div
                        key={grant.id}
                        className="flex items-center justify-between gap-2 rounded-md border border-border p-3 text-sm"
                      >
                        <span className="truncate">
                          {memberLabel(
                            members.find(
                              (member) => member.id === grant.memberId,
                            ),
                            t("unknownIdentity"),
                          )}
                        </span>
                        <span className="text-xs text-muted-foreground">
                          {t(grant.action)} · {t(grant.effect)}
                        </span>
                      </div>
                    ))
                : profiles
                    .filter((profile) =>
                      profile.groupIds.includes(detailGroup || ""),
                    )
                    .map((profile) => (
                      <div
                        key={profile.id}
                        className="flex items-center justify-between gap-2 rounded-md border border-border p-3 text-sm"
                      >
                        <span className="truncate">{profile.name}</span>
                      </div>
                    ))}
            </div>
          </ScrollArea>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDetailGroup(undefined)}>
              {t("close")}
            </Button>
            {detailGroup && (
              <Button
                onClick={() => {
                  const id = detailGroup;
                  setDetailGroup(undefined);
                  if (detailTab === "members") void open("access", [id]);
                  else {
                    setSelected([]);
                    setDetailGroup(undefined);
                  }
                }}
              >
                {t(detailTab === "members" ? "permissions" : "close")}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

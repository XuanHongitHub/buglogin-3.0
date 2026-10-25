"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { LuRefreshCw, LuUserPlus } from "react-icons/lu";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  Member,
  MemberRole,
  OrganizationMemberPage,
  Workspace,
} from "../../frontend/src/generated/command-registry";

type WorkspaceAccessRow = {
  workspace: Pick<Workspace, "id" | "name">;
  membership: Member | null;
};

const assignableRoles: MemberRole[] = ["admin", "lead", "operator", "member"];

export function OrganizationWorkspaceAccess({
  organizationId,
  workspaces,
  allowOwner = false,
  listOrganizationMembers,
  listWorkspaceMembers,
  assignMember,
}: {
  organizationId: string;
  workspaces: Array<Pick<Workspace, "id" | "name">>;
  allowOwner?: boolean;
  listOrganizationMembers: (
    organizationId: string,
  ) => Promise<OrganizationMemberPage>;
  listWorkspaceMembers: (workspaceId: string) => Promise<{ items: Member[] }>;
  assignMember: (
    input: {
      organizationId: string;
      workspaceId: string;
      organizationMemberId: string;
      organizationMemberRevision: number;
      role: MemberRole;
    },
    idempotencyKey: string,
  ) => Promise<Member>;
}) {
  const [members, setMembers] = useState<OrganizationMemberPage["items"]>([]);
  const [assignments, setAssignments] = useState<
    Map<string, WorkspaceAccessRow[]>
  >(new Map());
  const [selectedWorkspace, setSelectedWorkspace] = useState<
    Record<string, string>
  >({});
  const [selectedRole, setSelectedRole] = useState<Record<string, MemberRole>>(
    {},
  );
  const [busyMemberId, setBusyMemberId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [organizationPage, workspacePages] = await Promise.all([
        listOrganizationMembers(organizationId),
        Promise.all(
          workspaces.map((workspace) => listWorkspaceMembers(workspace.id)),
        ),
      ]);
      const activeWorkspaces = workspaces.map((workspace, index) => ({
        workspace,
        members: workspacePages[index].items,
      }));
      setMembers(organizationPage.items);
      setAssignments(
        new Map(
          organizationPage.items.map((organizationMember) => [
            organizationMember.id,
            activeWorkspaces.map(
              ({ workspace, members: workspaceMembers }) => ({
                workspace,
                membership:
                  workspaceMembers.find(
                    (member) =>
                      member.accountId === organizationMember.accountId,
                  ) ?? null,
              }),
            ),
          ]),
        ),
      );
    } catch (loadError) {
      setError(
        loadError instanceof Error ? loadError.message : String(loadError),
      );
    } finally {
      setLoading(false);
    }
  }, [
    listOrganizationMembers,
    listWorkspaceMembers,
    organizationId,
    workspaces,
  ]);

  useEffect(() => {
    void load();
  }, [load]);

  const activeMembers = useMemo(
    () => members.filter((member) => member.status === "active"),
    [members],
  );

  const assign = async (organizationMemberId: string) => {
    const organizationMember = activeMembers.find(
      (member) => member.id === organizationMemberId,
    );
    const workspaceId = selectedWorkspace[organizationMemberId];
    const role = selectedRole[organizationMemberId] ?? "member";
    if (!organizationMember || !workspaceId || busyMemberId) return;
    setBusyMemberId(organizationMemberId);
    setError(null);
    try {
      await assignMember(
        {
          organizationId,
          workspaceId,
          organizationMemberId,
          organizationMemberRevision: organizationMember.revision,
          role,
        },
        `organization-workspace-assign-${crypto.randomUUID()}`,
      );
      await load();
    } catch (assignError) {
      setError(
        assignError instanceof Error
          ? assignError.message
          : String(assignError),
      );
    } finally {
      setBusyMemberId(null);
    }
  };

  if (loading && members.length === 0) {
    return (
      <p role="status" className="text-sm text-muted-foreground">
        Loading workspace access…
      </p>
    );
  }

  return (
    <section aria-labelledby="workspace-access-heading" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h2 id="workspace-access-heading" className="text-sm font-semibold">
            Workspace access
          </h2>
          <p className="text-xs text-muted-foreground">
            Assign an active organization member to a workspace with a workspace
            role.
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void load()}
          disabled={loading}
        >
          <LuRefreshCw className="mr-1.5 size-3.5" /> Refresh
        </Button>
      </div>
      {error ? (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 p-3 text-sm"
        >
          <span>{error}</span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void load()}
            disabled={loading}
          >
            Retry
          </Button>
        </div>
      ) : null}
      {workspaces.length === 0 ? (
        <p className="rounded-md border border-border p-3 text-sm text-muted-foreground">
          No workspaces are available in this organization.
        </p>
      ) : members.length === 0 ? (
        <p className="rounded-md border border-border p-3 text-sm text-muted-foreground">
          No organization members are available to assign.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full min-w-[760px] text-left text-sm">
            <caption className="sr-only">
              Organization members and workspace assignments
            </caption>
            <thead className="border-b border-border bg-muted/40 text-xs text-muted-foreground">
              <tr>
                <th scope="col" className="px-3 py-2">
                  Person
                </th>
                <th scope="col" className="px-3 py-2">
                  Workspace assignments
                </th>
                <th scope="col" className="px-3 py-2">
                  Add workspace access
                </th>
              </tr>
            </thead>
            <tbody>
              {members.map((member) => {
                const rows = assignments.get(member.id) ?? [];
                const available = rows.filter((row) => !row.membership);
                return (
                  <tr
                    key={member.id}
                    className="border-b border-border last:border-0 align-top"
                  >
                    <th scope="row" className="px-3 py-3 font-medium">
                      <span className="block">
                        {member.username || member.email}
                      </span>
                      {member.username && member.email ? (
                        <span className="block text-xs font-normal text-muted-foreground">
                          {member.email}
                        </span>
                      ) : null}
                      {member.status === "suspended" ? (
                        <span className="mt-1 block text-xs text-destructive">
                          Organization access suspended
                        </span>
                      ) : null}
                    </th>
                    <td className="px-3 py-3">
                      {rows.some((row) => row.membership) ? (
                        <ul className="space-y-1">
                          {rows
                            .filter((row) => row.membership)
                            .map(({ workspace, membership }) => (
                              <li key={workspace.id}>
                                {workspace.name}{" "}
                                <span className="text-muted-foreground">
                                  · {membership?.role} · {membership?.status}
                                </span>
                              </li>
                            ))}
                        </ul>
                      ) : (
                        <span className="text-muted-foreground">
                          No workspace access
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-3">
                      {member.status !== "active" ? (
                        <span className="text-xs text-muted-foreground">
                          Reactivate organization access first.
                        </span>
                      ) : available.length === 0 ? (
                        <span className="text-xs text-muted-foreground">
                          Assigned to every workspace.
                        </span>
                      ) : (
                        <div className="flex min-w-[310px] flex-wrap items-center gap-2">
                          <Select
                            value={
                              selectedWorkspace[member.id] ??
                              available[0]?.workspace.id
                            }
                            onValueChange={(value) =>
                              setSelectedWorkspace((current) => ({
                                ...current,
                                [member.id]: value,
                              }))
                            }
                          >
                            <SelectTrigger
                              aria-label={`Workspace for ${member.username || member.email}`}
                              className="w-[170px]"
                            >
                              <SelectValue placeholder="Select workspace" />
                            </SelectTrigger>
                            <SelectContent>
                              {available.map(({ workspace }) => (
                                <SelectItem
                                  key={workspace.id}
                                  value={workspace.id}
                                >
                                  {workspace.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <Select
                            value={selectedRole[member.id] ?? "member"}
                            onValueChange={(value) =>
                              setSelectedRole((current) => ({
                                ...current,
                                [member.id]: value as MemberRole,
                              }))
                            }
                          >
                            <SelectTrigger
                              aria-label={`Role for ${member.username || member.email}`}
                              className="w-[105px]"
                            >
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              {(allowOwner
                                ? ["owner", ...assignableRoles]
                                : assignableRoles
                              ).map((role) => (
                                <SelectItem key={role} value={role}>
                                  {role}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <Button
                            type="button"
                            size="sm"
                            onClick={() => void assign(member.id)}
                            disabled={busyMemberId !== null}
                          >
                            <LuUserPlus className="mr-1.5 size-3.5" />{" "}
                            {busyMemberId === member.id
                              ? "Assigning…"
                              : "Assign"}
                          </Button>
                        </div>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

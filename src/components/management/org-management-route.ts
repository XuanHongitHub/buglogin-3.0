export type OrganizationTab = "workspaces" | "accounts" | "settings";

export type OrganizationManagementRoute = {
  organizationId: string | null;
  workspaceId: string | null;
  organizationTab: OrganizationTab;
  workspaceTab?: "overview" | "members" | "groups" | "profiles" | "resources";
};

export type PendingTenancyDelete =
  | {
      kind: "organization";
      organizationId: string;
      organizationName: string;
    }
  | {
      kind: "workspace";
      organizationId: string;
      organizationName: string;
      workspaceId: string;
      workspaceName: string;
    };

function normalizePendingTenancyDelete(
  parsed: unknown,
): PendingTenancyDelete | null {
  try {
    if (!parsed || typeof parsed !== "object") return null;
    const candidate = parsed as Record<string, unknown>;
    if (
      candidate.kind === "organization" &&
      typeof candidate.organizationId === "string" &&
      candidate.organizationId.length > 0 &&
      typeof candidate.organizationName === "string" &&
      candidate.organizationName.length > 0
    ) {
      return {
        kind: "organization",
        organizationId: candidate.organizationId,
        organizationName: candidate.organizationName,
      };
    }
    if (
      candidate.kind === "workspace" &&
      typeof candidate.organizationId === "string" &&
      candidate.organizationId.length > 0 &&
      typeof candidate.organizationName === "string" &&
      candidate.organizationName.length > 0 &&
      typeof candidate.workspaceId === "string" &&
      candidate.workspaceId.length > 0 &&
      typeof candidate.workspaceName === "string" &&
      candidate.workspaceName.length > 0
    ) {
      return {
        kind: "workspace",
        organizationId: candidate.organizationId,
        organizationName: candidate.organizationName,
        workspaceId: candidate.workspaceId,
        workspaceName: candidate.workspaceName,
      };
    }
  } catch {}
  return null;
}

function pendingTenancyDeleteKey(pending: PendingTenancyDelete): string {
  return `${pending.kind}:${pending.organizationId}:${pending.kind === "workspace" ? pending.workspaceId : ""}`;
}

export function parsePendingTenancyDeletes(
  value: string | null,
): PendingTenancyDelete[] {
  if (!value) return [];
  try {
    const parsed: unknown = JSON.parse(value);
    const records = Array.isArray(parsed) ? parsed : [parsed];
    const unique = new Map<string, PendingTenancyDelete>();
    for (const record of records) {
      const pending = normalizePendingTenancyDelete(record);
      if (pending) unique.set(pendingTenancyDeleteKey(pending), pending);
    }
    return [...unique.values()];
  } catch {
    return [];
  }
}

export function serializePendingTenancyDeletes(
  pending: PendingTenancyDelete[],
): string {
  return JSON.stringify(pending);
}

export function upsertPendingTenancyDelete(
  current: PendingTenancyDelete[],
  next: PendingTenancyDelete,
): PendingTenancyDelete[] {
  const key = pendingTenancyDeleteKey(next);
  return [
    ...current.filter((pending) => pendingTenancyDeleteKey(pending) !== key),
    next,
  ];
}

export function removePendingTenancyDelete(
  current: PendingTenancyDelete[],
  target: PendingTenancyDelete,
): PendingTenancyDelete[] {
  const key = pendingTenancyDeleteKey(target);
  return current.filter((pending) => pendingTenancyDeleteKey(pending) !== key);
}

const ORGANIZATION_TABS = new Set<OrganizationTab>([
  "workspaces",
  "accounts",
  "settings",
]);
const WORKSPACE_TABS = new Set([
  "overview",
  "members",
  "groups",
  "profiles",
  "resources",
]);

export function parseOrganizationManagementRoute(
  search: string,
): OrganizationManagementRoute | null {
  const params = new URLSearchParams(search);
  const hasManagementRoute = [
    "managementOrganization",
    "managementWorkspace",
    "managementTab",
    "managementWorkspaceTab",
  ].some((key) => params.has(key));
  if (!hasManagementRoute) return null;

  const tab = params.get("managementTab") as OrganizationTab | null;
  const workspaceTab = params.get("managementWorkspaceTab");
  return {
    organizationId: params.get("managementOrganization") || null,
    workspaceId: params.get("managementWorkspace") || null,
    organizationTab: tab && ORGANIZATION_TABS.has(tab) ? tab : "workspaces",
    workspaceTab:
      workspaceTab && WORKSPACE_TABS.has(workspaceTab)
        ? (workspaceTab as
            | "overview"
            | "members"
            | "groups"
            | "profiles"
            | "resources")
        : undefined,
  };
}

export function serializeOrganizationManagementRoute(
  search: string,
  route: OrganizationManagementRoute,
): string {
  const params = new URLSearchParams(search);
  params.delete("managementOrganization");
  params.delete("managementWorkspace");
  params.delete("managementTab");
  params.delete("managementWorkspaceTab");
  if (route.organizationId)
    params.set("managementOrganization", route.organizationId);
  if (route.workspaceId) params.set("managementWorkspace", route.workspaceId);
  if (route.organizationId) params.set("managementTab", route.organizationTab);
  if (
    route.workspaceId &&
    route.workspaceTab &&
    route.workspaceTab !== "overview"
  )
    params.set("managementWorkspaceTab", route.workspaceTab);
  return params.toString();
}

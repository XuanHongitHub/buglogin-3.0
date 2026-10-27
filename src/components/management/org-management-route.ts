export type OrganizationTab = "workspaces" | "accounts" | "settings";

export type OrganizationManagementRoute = {
  organizationId: string | null;
  workspaceId: string | null;
  organizationTab: OrganizationTab;
  workspaceTab?: "overview" | "members" | "groups" | "profiles" | "resources";
};

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

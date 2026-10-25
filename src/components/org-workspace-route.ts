export type OrgWorkspaceSection =
  | "overview"
  | "members"
  | "groups"
  | "profiles";

export interface OrgWorkspaceRoute {
  organizationId?: string;
  workspaceId?: string;
  section?: OrgWorkspaceSection;
  groupId?: string;
  profileId?: string;
}

const SECTIONS = new Set<OrgWorkspaceSection>([
  "overview",
  "members",
  "groups",
  "profiles",
]);

export function parseOrgWorkspaceRoute(search: string): OrgWorkspaceRoute {
  const params = new URLSearchParams(search);
  const section = params.get("section") as OrgWorkspaceSection | null;
  return {
    organizationId: params.get("org") || undefined,
    workspaceId: params.get("workspace") || undefined,
    section: section && SECTIONS.has(section) ? section : undefined,
    groupId: params.get("group") || undefined,
    profileId: params.get("profile") || undefined,
  };
}

export function serializeOrgWorkspaceRoute(
  currentSearch: string,
  route: OrgWorkspaceRoute,
): string {
  const query = serializeOrgWorkspaceSearch(currentSearch, route);
  return `${window.location.pathname}${query ? `?${query}` : ""}${window.location.hash}`;
}

export function serializeOrgWorkspaceSearch(
  currentSearch: string,
  route: OrgWorkspaceRoute,
): string {
  const params = new URLSearchParams(currentSearch);
  for (const key of ["org", "workspace", "section", "group", "profile"])
    params.delete(key);
  if (route.organizationId) params.set("org", route.organizationId);
  if (route.workspaceId) params.set("workspace", route.workspaceId);
  if (route.section) params.set("section", route.section);
  if (route.groupId) params.set("group", route.groupId);
  if (route.profileId) params.set("profile", route.profileId);
  return params.toString();
}

export function sameOrgWorkspaceRoute(
  left: OrgWorkspaceRoute,
  right: OrgWorkspaceRoute,
): boolean {
  return (
    left.organizationId === right.organizationId &&
    left.workspaceId === right.workspaceId &&
    left.section === right.section &&
    left.groupId === right.groupId &&
    left.profileId === right.profileId
  );
}

export type DeleteResolution =
  | {
      kind: "navigate";
      destination:
        | "profiles"
        | "resources"
        | "groups"
        | "members"
        | "workspaces"
        | "accounts";
    }
  | { kind: "refresh" }
  | { kind: "unknown" };

const WORKSPACE_DELETE_RESOLUTIONS: Record<string, DeleteResolution> = {
  workspace_has_profiles: { kind: "navigate", destination: "profiles" },
  workspace_has_resources: { kind: "navigate", destination: "resources" },
  workspace_has_groups: { kind: "navigate", destination: "groups" },
  workspace_has_teams: { kind: "navigate", destination: "members" },
  workspace_has_members: { kind: "navigate", destination: "members" },
  workspace_has_active_operations: { kind: "refresh" },
  workspace_has_sync_reservations: { kind: "refresh" },
};

const ORGANIZATION_DELETE_RESOLUTIONS: Record<string, DeleteResolution> = {
  organization_not_empty: { kind: "navigate", destination: "workspaces" },
  organization_has_members: { kind: "navigate", destination: "accounts" },
};

export function resolveWorkspaceDeleteBlocker(code: string): DeleteResolution {
  return WORKSPACE_DELETE_RESOLUTIONS[code] ?? { kind: "unknown" };
}

export function resolveOrganizationDeleteBlocker(
  code: string,
): DeleteResolution {
  return ORGANIZATION_DELETE_RESOLUTIONS[code] ?? { kind: "unknown" };
}

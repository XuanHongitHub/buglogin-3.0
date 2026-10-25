export function canOpenOrganizationMembers(input: {
  platformAdmin: boolean;
  organizationOwner: boolean;
  workspaceRoles: readonly string[];
}): boolean {
  return (
    input.platformAdmin ||
    input.organizationOwner ||
    input.workspaceRoles.some((role) => role === "admin" || role === "lead")
  );
}

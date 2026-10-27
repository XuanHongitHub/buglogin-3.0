export type AccountDirectoryStatusScope =
  | "platform"
  | "organization"
  | "workspace";

export type AccountDirectoryStatusInput = {
  accountStatus?: string;
  organizationMembershipStatus?: string;
  workspaceMembershipStatus?: string;
};

export type AccountDirectoryStatusRow = {
  label: "accountStatus" | "organizationMembership" | "workspaceMembership";
  value: string | null;
};

/** Keeps account state separate from membership state; missing data stays unknown. */
export function accountDirectoryStatuses(
  scope: AccountDirectoryStatusScope,
  input: AccountDirectoryStatusInput,
): AccountDirectoryStatusRow[] {
  if (scope === "organization") {
    return [
      { label: "accountStatus", value: input.accountStatus ?? null },
      {
        label: "organizationMembership",
        value: input.organizationMembershipStatus ?? null,
      },
    ];
  }
  if (scope === "workspace") {
    return [
      {
        label: "workspaceMembership",
        value: input.workspaceMembershipStatus ?? null,
      },
    ];
  }
  return [{ label: "accountStatus", value: input.accountStatus ?? null }];
}

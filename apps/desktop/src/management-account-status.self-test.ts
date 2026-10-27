import assert from "node:assert/strict";
import { accountDirectoryStatuses } from "./management-account-status.ts";

assert.deepEqual(
  accountDirectoryStatuses("organization", {
    organizationMembershipStatus: "active",
  }),
  [
    { label: "accountStatus", value: null },
    { label: "organizationMembership", value: "active" },
  ],
  "organization membership must not be mistaken for account status",
);

assert.deepEqual(
  accountDirectoryStatuses("workspace", {
    workspaceMembershipStatus: "suspended",
  }),
  [{ label: "workspaceMembership", value: "suspended" }],
);

assert.deepEqual(
  accountDirectoryStatuses("platform", { accountStatus: "disabled" }),
  [{ label: "accountStatus", value: "disabled" }],
);

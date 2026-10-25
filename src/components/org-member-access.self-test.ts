import assert from "node:assert/strict";
import { canOpenOrganizationMembers } from "./org-member-access.ts";

assert.equal(
  canOpenOrganizationMembers({
    platformAdmin: false,
    organizationOwner: false,
    workspaceRoles: ["member"],
  }),
  false,
);
for (const role of ["admin", "lead"]) {
  assert.equal(
    canOpenOrganizationMembers({
      platformAdmin: false,
      organizationOwner: false,
      workspaceRoles: [role],
    }),
    true,
  );
}
assert.equal(
  canOpenOrganizationMembers({
    platformAdmin: false,
    organizationOwner: true,
    workspaceRoles: [],
  }),
  true,
);
assert.equal(
  canOpenOrganizationMembers({
    platformAdmin: true,
    organizationOwner: false,
    workspaceRoles: [],
  }),
  true,
);

console.log("organization member access hint self-test passed");

import assert from "node:assert/strict";

import {
  resolveOrganizationDeleteBlocker,
  resolveWorkspaceDeleteBlocker,
} from "./delete-resolution.ts";

assert.deepEqual(resolveWorkspaceDeleteBlocker("workspace_has_profiles"), {
  kind: "navigate",
  destination: "profiles",
});
assert.deepEqual(resolveWorkspaceDeleteBlocker("workspace_has_resources"), {
  kind: "navigate",
  destination: "resources",
});
assert.deepEqual(resolveWorkspaceDeleteBlocker("workspace_has_groups"), {
  kind: "navigate",
  destination: "groups",
});
assert.deepEqual(resolveWorkspaceDeleteBlocker("workspace_has_teams"), {
  kind: "navigate",
  destination: "members",
});
assert.deepEqual(resolveWorkspaceDeleteBlocker("workspace_has_members"), {
  kind: "navigate",
  destination: "members",
});
assert.deepEqual(
  resolveWorkspaceDeleteBlocker("workspace_has_active_operations"),
  { kind: "refresh" },
);
assert.deepEqual(
  resolveWorkspaceDeleteBlocker("workspace_has_sync_reservations"),
  { kind: "refresh" },
);
assert.deepEqual(resolveOrganizationDeleteBlocker("organization_not_empty"), {
  kind: "navigate",
  destination: "workspaces",
});
assert.deepEqual(resolveOrganizationDeleteBlocker("organization_has_members"), {
  kind: "navigate",
  destination: "accounts",
});
assert.deepEqual(resolveWorkspaceDeleteBlocker("future_blocker"), {
  kind: "unknown",
});
assert.deepEqual(resolveOrganizationDeleteBlocker("future_blocker"), {
  kind: "unknown",
});

console.log("management delete resolution self-test passed");

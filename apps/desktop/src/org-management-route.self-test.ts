import assert from "node:assert/strict";
import {
  parseOrganizationManagementRoute,
  parsePendingTenancyDeletes,
  removePendingTenancyDelete,
  serializeOrganizationManagementRoute,
  serializePendingTenancyDeletes,
  upsertPendingTenancyDelete,
} from "../../../src/components/management/org-management-route.ts";

assert.equal(parseOrganizationManagementRoute("?theme=dark"), null);
assert.deepEqual(
  parseOrganizationManagementRoute(
    "?theme=dark&managementOrganization=org_1&managementWorkspace=ws_2&managementTab=accounts&managementWorkspaceTab=profiles",
  ),
  {
    organizationId: "org_1",
    workspaceId: "ws_2",
    organizationTab: "accounts",
    workspaceTab: "profiles",
  },
);
assert.equal(
  parseOrganizationManagementRoute(
    "?managementOrganization=org_1&managementTab=unknown",
  )?.organizationTab,
  "workspaces",
);
assert.equal(
  serializeOrganizationManagementRoute(
    "?theme=dark&managementOrganization=old",
    {
      organizationId: "org_next",
      workspaceId: "ws_next",
      organizationTab: "settings",
      workspaceTab: "groups",
    },
  ),
  "theme=dark&managementOrganization=org_next&managementWorkspace=ws_next&managementTab=settings&managementWorkspaceTab=groups",
);
assert.equal(
  serializeOrganizationManagementRoute("?managementOrganization=old", {
    organizationId: null,
    workspaceId: null,
    organizationTab: "workspaces",
  }),
  "",
);
assert.equal(
  parseOrganizationManagementRoute(
    "?managementOrganization=org_1&managementWorkspace=ws_1&managementWorkspaceTab=resources",
  )?.workspaceTab,
  "resources",
);

const pendingDelete = {
  kind: "workspace" as const,
  organizationId: "org_1",
  organizationName: "Partner BugMedia",
  workspaceId: "ws_2",
  workspaceName: "Workspace 100/30",
};
assert.deepEqual(
  parsePendingTenancyDeletes(serializePendingTenancyDeletes([pendingDelete])),
  [pendingDelete],
);
assert.deepEqual(parsePendingTenancyDeletes("not-json"), []);
assert.deepEqual(
  parsePendingTenancyDeletes(
    JSON.stringify({ kind: "organization", organizationId: "org_1" }),
  ),
  [],
);
const pendingOrganization = {
  kind: "organization" as const,
  organizationId: "org_1",
  organizationName: "Partner BugMedia",
};
const pendingDeletes = upsertPendingTenancyDelete(
  [pendingOrganization],
  pendingDelete,
);
assert.deepEqual(pendingDeletes, [pendingOrganization, pendingDelete]);
assert.deepEqual(removePendingTenancyDelete(pendingDeletes, pendingDelete), [
  pendingOrganization,
]);

console.log("organization management route self-test passed");

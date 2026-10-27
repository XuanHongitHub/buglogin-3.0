import assert from "node:assert/strict";
import {
  parseOrganizationManagementRoute,
  serializeOrganizationManagementRoute,
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

console.log("organization management route self-test passed");

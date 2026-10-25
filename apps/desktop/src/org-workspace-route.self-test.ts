import assert from "node:assert/strict";
import {
  parseOrgWorkspaceRoute,
  sameOrgWorkspaceRoute,
  serializeOrgWorkspaceSearch,
} from "../../../src/components/org-workspace-route.ts";

assert.deepEqual(
  parseOrgWorkspaceRoute(
    "?org=org_1&workspace=ws_2&section=profiles&profile=prf_3&keep=1",
  ),
  {
    organizationId: "org_1",
    workspaceId: "ws_2",
    section: "profiles",
    profileId: "prf_3",
    groupId: undefined,
  },
);
assert.equal(
  parseOrgWorkspaceRoute("?section=unknown&workspace=ws_2").section,
  undefined,
);
assert.equal(
  sameOrgWorkspaceRoute(
    { organizationId: "org_1", workspaceId: "ws_2", section: "groups" },
    { organizationId: "org_1", workspaceId: "ws_2", section: "groups" },
  ),
  true,
);
assert.equal(
  sameOrgWorkspaceRoute(
    { workspaceId: "ws_2", section: "groups" },
    { workspaceId: "ws_2", section: "profiles" },
  ),
  false,
);
assert.equal(
  serializeOrgWorkspaceSearch("?keep=1&profile=old&org=old", {
    organizationId: "org_next",
    workspaceId: "ws_next",
    section: "groups",
    groupId: "grp_next",
  }),
  "keep=1&org=org_next&workspace=ws_next&section=groups&group=grp_next",
);

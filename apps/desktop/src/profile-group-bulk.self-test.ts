import assert from "node:assert/strict";
import { replaceGroupsForProfiles } from "./profile-group-bulk.ts";

const calls: Array<Record<string, unknown>> = [];
const result = await replaceGroupsForProfiles({
  workspaceId: "workspace-a",
  profiles: [
    { id: "profile-a", name: "Profile A" },
    { id: "profile-b", name: "Profile B" },
  ],
  groupIds: ["group-a"],
  api: {
    async getProfile({ profileId }) {
      if (profileId === "profile-b") throw new Error("profile_forbidden");
      return { id: profileId, name: "Profile A", revision: 7 };
    },
    async replaceProfileGroups(input) {
      calls.push(input);
      return { id: input.profileId, groupIds: input.groupIds };
    },
  },
});

assert.deepEqual(result.succeededIds, ["profile-a"]);
assert.deepEqual(result.failures, [
  { profileId: "profile-b", name: "Profile B", error: "profile_forbidden" },
]);
assert.deepEqual(calls, [
  {
    workspaceId: "workspace-a",
    profileId: "profile-a",
    expectedRevision: 7,
    groupIds: ["group-a"],
  },
]);

console.log("Profile group bulk assignment self-test passed");

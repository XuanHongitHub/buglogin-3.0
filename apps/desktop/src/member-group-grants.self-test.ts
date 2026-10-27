import assert from "node:assert/strict";
import {
  type DirectGroupGrant,
  previewDirectGroupGrants,
} from "./member-group-grants.ts";

const current: DirectGroupGrant[] = [
  { groupId: "keep", action: "update" },
  { groupId: "keep", action: "read" },
  { groupId: "remove", action: "execute" },
];

assert.deepEqual(previewDirectGroupGrants(current, "add", ["keep", "new"]), [
  { groupId: "keep", action: "execute" },
  { groupId: "keep", action: "read" },
  { groupId: "keep", action: "update" },
  { groupId: "new", action: "execute" },
  { groupId: "new", action: "read" },
  { groupId: "remove", action: "execute" },
]);
assert.deepEqual(previewDirectGroupGrants(current, "replace", ["keep"]), [
  { groupId: "keep", action: "execute" },
  { groupId: "keep", action: "read" },
  { groupId: "keep", action: "update" },
]);
assert.deepEqual(previewDirectGroupGrants(current, "remove", ["remove"]), [
  { groupId: "keep", action: "read" },
  { groupId: "keep", action: "update" },
]);

console.log("Member group grant preview self-test passed");

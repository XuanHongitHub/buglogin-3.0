import assert from "node:assert/strict";
import {
  appendResourceCursorPage,
  RESOURCE_INVENTORY_PAGE_SIZE,
  resourceListPageInput,
} from "./resource-inventory-pagination.ts";

const firstPage = [{ cursor: undefined }];
assert.deepEqual(appendResourceCursorPage(firstPage, 0, "cursor-a"), [
  { cursor: undefined },
  { cursor: "cursor-a" },
]);
assert.equal(appendResourceCursorPage(firstPage, 0, null), null);
assert.equal(
  appendResourceCursorPage(
    [{ cursor: undefined }, { cursor: "cursor-a" }],
    1,
    "cursor-a",
  ),
  null,
);
assert.deepEqual(resourceListPageInput("workspace-a"), {
  workspaceId: "workspace-a",
  limit: RESOURCE_INVENTORY_PAGE_SIZE,
});
assert.deepEqual(resourceListPageInput("workspace-a", "cursor-a"), {
  workspaceId: "workspace-a",
  limit: RESOURCE_INVENTORY_PAGE_SIZE,
  cursor: "cursor-a",
});

console.log("Resource inventory pagination self-test passed");

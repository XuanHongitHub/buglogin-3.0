import assert from "node:assert/strict";
import { awaitDesktopOperation } from "./await-desktop-operation.ts";

const initial = {
  operationId: "op_stop",
  commandId: "profile.stop",
  status: "accepted" as const,
};
let calls = 0;
const completed = await awaitDesktopOperation(
  initial,
  async (operationId) => {
    assert.equal(operationId, initial.operationId);
    calls += 1;
    return calls === 1
      ? { operationId, status: "running" as const }
      : { operationId, status: "completed" as const };
  },
  { intervalMs: 0 },
);
assert.equal(completed.status, "completed");
assert.equal(calls, 2);

await assert.rejects(
  awaitDesktopOperation(
    initial,
    async (operationId) => ({
      operationId,
      status: "failed" as const,
      error: {
        type: "operation",
        title: "Sync failed",
        status: 503,
        code: "sync_failed",
        correlationId: "test",
        retryable: true,
        fields: {},
      },
    }),
    { intervalMs: 0 },
  ).then((operation) => {
    if (operation.status === "failed") throw new Error(operation.error?.title);
  }),
  /Sync failed/,
);

await assert.rejects(
  awaitDesktopOperation(
    initial,
    async (operationId) => ({ operationId, status: "running" }),
    {
      intervalMs: 0,
      maxAttempts: 2,
    },
  ),
  /still in progress/,
);

console.log("desktop operation polling self-test passed");

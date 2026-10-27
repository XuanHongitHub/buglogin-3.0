import assert from "node:assert/strict";
import { resolveTeamLockState } from "./team-lock-state.ts";

const currentTime = 1_800_000_000_000;

const delayedLease = {
  ownerId: "account-2",
  issuedAtMs: currentTime - 40_000,
  expiresAtMs: currentTime + 20_000,
};
const delayedRuntime = {
  deviceId: "device-2",
  ownerId: "account-2",
  state: "running" as const,
  updatedAtMs: currentTime - 20_000,
};
assert.deepEqual(
  resolveTeamLockState(
    delayedLease,
    delayedRuntime,
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: true,
    syncingRemotely: false,
  },
  "a live lease and matching running projection are a confirmed remote lock",
);
assert.deepEqual(
  resolveTeamLockState(
    delayedLease,
    { ...delayedRuntime, state: "stopped" },
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: false,
    syncingRemotely: false,
  },
  "a stopped projection with a retained lease is a lock, not assumed syncing",
);
assert.deepEqual(
  resolveTeamLockState(
    delayedLease,
    { ...delayedRuntime, state: "syncing" },
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: false,
    syncingRemotely: true,
  },
  "only an explicit syncing projection is displayed as syncing",
);
assert.deepEqual(
  resolveTeamLockState(
    { ...delayedLease, expiresAtMs: currentTime - 1 },
    delayedRuntime,
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: false,
    syncingRemotely: false,
  },
  "the UI trusts the server-filtered lease instead of comparing client clocks",
);

assert.deepEqual(
  resolveTeamLockState(null, null, null, false, currentTime),
  {
    unknown: false,
    lockedRemotely: false,
    runningRemotely: false,
    syncingRemotely: false,
  },
  "an absent server lease unlocks the profile even without runtime data",
);

assert.deepEqual(
  resolveTeamLockState(
    {
      ownerId: "account-1",
      issuedAtMs: currentTime - 10_000,
      expiresAtMs: currentTime + 60_000,
    },
    null,
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: false,
    syncingRemotely: false,
  },
  "a live lease remains a confirmed lock even when activity details are unavailable",
);

assert.deepEqual(
  resolveTeamLockState(
    {
      ownerId: "account-2",
      issuedAtMs: currentTime - 10_000,
      expiresAtMs: currentTime + 60_000,
    },
    {
      deviceId: "device-2",
      ownerId: "account-2",
      state: "running",
      updatedAtMs: currentTime,
    },
    "device-1",
    false,
    currentTime,
  ),
  {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: true,
    syncingRemotely: false,
  },
  "a fresh lease and matching process on another device are shown as remote",
);

import assert from "node:assert/strict";
import { resolveTeamLockState } from "./team-lock-state.ts";

const currentTime = 1_800_000_000_000;

assert.deepEqual(
  resolveTeamLockState(null, null, null, false, currentTime),
  { unknown: false, runningRemotely: false },
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
  { unknown: true, runningRemotely: false },
  "a lease without process evidence remains blocked",
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
  { unknown: false, runningRemotely: true },
  "a fresh lease and matching process on another device are shown as remote",
);

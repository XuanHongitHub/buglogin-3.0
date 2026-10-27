export type TeamLeaseSnapshot = {
  ownerId: string;
  issuedAtMs: number;
  expiresAtMs: number;
};

export type TeamRuntimeSnapshot = {
  deviceId: string;
  ownerId: string;
  ownerEmail?: string | null;
  state: "running" | "syncing" | "stopped";
  updatedAtMs: number;
} | null;

export function resolveTeamLockState(
  lease: TeamLeaseSnapshot | null,
  runtime: TeamRuntimeSnapshot,
  localDeviceId: string | null,
  locallyRunning: boolean,
  _nowMs = Date.now(),
): {
  unknown: boolean;
  lockedRemotely: boolean;
  runningRemotely: boolean;
  syncingRemotely: boolean;
} {
  // The coordination lease is the launch authority. With no lease there is
  // no team lock, even if the informational runtime projection is unavailable.
  if (!lease) {
    return {
      unknown: false,
      lockedRemotely: false,
      runningRemotely: false,
      syncingRemotely: false,
    };
  }

  // The coordination API filters expired leases using server time. Do not
  // reinterpret a confirmed lease with the desktop clock, which may be skewed.
  const runtimeMatchesLease =
    runtime?.ownerId === lease.ownerId && Boolean(runtime.deviceId);

  if (runtimeMatchesLease && runtime?.state === "syncing") {
    return {
      unknown: false,
      lockedRemotely: true,
      runningRemotely: false,
      syncingRemotely: true,
    };
  }

  if (runtimeMatchesLease && runtime?.state === "running") {
    const runningRemotely =
      localDeviceId !== null && runtime.deviceId !== localDeviceId;
    return {
      unknown: false,
      lockedRemotely: runningRemotely || !locallyRunning,
      runningRemotely,
      syncingRemotely: false,
    };
  }

  // A live lease is definitive lock evidence. Missing/stale runtime detail
  // affects the activity label, not whether a second device may use the data.
  return {
    unknown: false,
    lockedRemotely: true,
    runningRemotely: false,
    syncingRemotely: false,
  };
}

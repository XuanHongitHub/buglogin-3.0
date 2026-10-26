const RUNTIME_LEASE_TTL_MS = 60_000;
const RUNTIME_HEARTBEAT_INTERVAL_MS = 15_000;
const RUNTIME_HEARTBEAT_JITTER_MS = 3_000;
const MIN_LEASE_REMAINING_FOR_RUNNING_MS =
  RUNTIME_LEASE_TTL_MS -
  RUNTIME_HEARTBEAT_INTERVAL_MS -
  RUNTIME_HEARTBEAT_JITTER_MS;

export type TeamLeaseSnapshot = {
  ownerId: string;
  issuedAtMs: number;
  expiresAtMs: number;
};

export type TeamRuntimeSnapshot = {
  deviceId: string;
  ownerId: string;
  ownerEmail?: string | null;
  state: "running" | "stopped";
  updatedAtMs: number;
} | null;

export function resolveTeamLockState(
  lease: TeamLeaseSnapshot | null,
  runtime: TeamRuntimeSnapshot,
  localDeviceId: string | null,
  locallyRunning: boolean,
  nowMs = Date.now(),
): { unknown: boolean; runningRemotely: boolean } {
  // The coordination lease is the launch authority. With no lease there is
  // no team lock, even if the informational runtime projection is unavailable.
  if (!lease) return { unknown: false, runningRemotely: false };

  const recentlyRenewed =
    lease.expiresAtMs - nowMs > MIN_LEASE_REMAINING_FOR_RUNNING_MS;
  const runtimeMatchesLease =
    runtime?.ownerId === lease.ownerId && Boolean(runtime.deviceId);

  if (
    lease &&
    runtime?.state === "running" &&
    runtimeMatchesLease &&
    recentlyRenewed
  ) {
    if (
      localDeviceId === null ||
      (runtime.deviceId === localDeviceId && !locallyRunning)
    ) {
      return { unknown: true, runningRemotely: false };
    }
    return { unknown: false, runningRemotely: true };
  }

  // A live lease without matching process evidence may be protecting sync or
  // a crashed device. Keep it blocked until expiry/reconciliation.
  return { unknown: true, runningRemotely: false };
}

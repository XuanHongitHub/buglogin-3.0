import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ProfileLockInfo } from "@/types";

const BUGLOGIN_DESKTOP =
  process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop";
const RUNTIME_LEASE_TTL_MS = 120_000;
const RUNTIME_HEARTBEAT_INTERVAL_MS = 30_000;
const RUNTIME_HEARTBEAT_JITTER_MS = 5_000;
const MIN_LEASE_REMAINING_FOR_RUNNING_MS =
  RUNTIME_LEASE_TTL_MS -
  RUNTIME_HEARTBEAT_INTERVAL_MS -
  RUNTIME_HEARTBEAT_JITTER_MS;

type CommandResult<T> = { data: T };
type ProfileBinding = {
  profileId: string;
  resourceId: string;
  workspaceId: string;
  archiveState: string;
};
type LeaseStatus = {
  ownerId: string;
  issuedAtMs: number;
  expiresAtMs: number;
};
type RuntimeProjection = {
  deviceId: string;
  ownerId: string;
  ownerEmail?: string | null;
  state: "running" | "stopped";
  updatedAtMs: number;
} | null;

export function useTeamLocks(
  currentUserId?: string,
  workspaceId?: string | null,
  profiles: Array<{ id: string }> = [],
  locallyRunningProfileIds: ReadonlySet<string> = new Set(),
) {
  const [locks, setLocks] = useState<ProfileLockInfo[]>([]);
  const [unknownProfileIds, setUnknownProfileIds] = useState<Set<string>>(
    new Set(),
  );
  const [runtimeRunningProfileIds, setRuntimeRunningProfileIds] = useState<
    Set<string>
  >(new Set());
  const [localDeviceId, setLocalDeviceId] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const profileIds = useMemo(
    () => profiles.map((profile) => profile.id),
    [profiles],
  );

  const fetchLocks = useCallback(async () => {
    const sequence = ++requestSequence.current;
    if (BUGLOGIN_DESKTOP) {
      if (!workspaceId || profileIds.length === 0) {
        setLocks([]);
        setUnknownProfileIds(new Set());
        setRuntimeRunningProfileIds(new Set());
        return;
      }
      const nextLocks: ProfileLockInfo[] = [];
      const checked = new Set<string>();
      const unconfirmed = new Set<string>();
      const runtimeRunning = new Set<string>();
      await Promise.all(
        profileIds.map(async (profileId) => {
          try {
            const correlationId = `lease-status-${crypto.randomUUID()}`;
            const bindingResult = await invoke<CommandResult<ProfileBinding>>(
              "profile_binding_get",
              { correlationId, payload: { profileId } },
            );
            const binding = bindingResult.data;
            if (
              binding.archiveState !== "active" ||
              binding.workspaceId !== workspaceId
            ) {
              checked.add(profileId);
              return;
            }
            const [leaseResult, runtimeResult] = await Promise.all([
              invoke<CommandResult<LeaseStatus | null>>("resource_lease_get", {
                correlationId: `lease-status-${crypto.randomUUID()}`,
                payload: {
                  workspaceId: binding.workspaceId,
                  resourceId: binding.resourceId,
                },
              }),
              invoke<CommandResult<RuntimeProjection>>("profile_runtime_get", {
                correlationId: `runtime-status-${crypto.randomUUID()}`,
                payload: { profileId },
              }),
            ]);
            const lease = leaseResult.data;
            const runtime = runtimeResult.data;
            checked.add(profileId);
            const recentlyRenewed =
              lease !== null &&
              lease.expiresAtMs - Date.now() >
                MIN_LEASE_REMAINING_FOR_RUNNING_MS;
            const runtimeMatchesLease =
              lease !== null &&
              runtime?.ownerId === lease.ownerId &&
              Boolean(runtime.deviceId);
            if (
              lease &&
              runtime?.state === "running" &&
              runtimeMatchesLease &&
              recentlyRenewed
            ) {
              if (
                localDeviceId === null ||
                (runtime.deviceId === localDeviceId &&
                  !locallyRunningProfileIds.has(profileId))
              ) {
                unconfirmed.add(profileId);
                return;
              }
              runtimeRunning.add(profileId);
              nextLocks.push({
                profileId,
                lockedBy: lease.ownerId,
                lockedByEmail: runtime.ownerEmail ?? undefined,
                ownerLabel: runtime.ownerEmail ?? undefined,
                lockedAt: new Date(lease.issuedAtMs).toISOString(),
                expiresAt: new Date(lease.expiresAtMs).toISOString(),
              });
            } else if (lease) {
              // A lease can outlive a crashed app or protect an in-progress sync.
              // It blocks relaunch, but does not prove a browser process is alive.
              unconfirmed.add(profileId);
            }
          } catch {
            // A failed read is unknown, not evidence of an active lock.
          }
        }),
      );
      // The server is authoritative; a failed read must not leave stale locks visible.
      if (sequence !== requestSequence.current) return;
      setLocks(nextLocks);
      setUnknownProfileIds(
        new Set([
          ...profileIds.filter((profileId) => !checked.has(profileId)),
          ...unconfirmed,
        ]),
      );
      setRuntimeRunningProfileIds(runtimeRunning);
      return;
    }
    try {
      const result = await invoke<ProfileLockInfo[]>("get_team_locks");
      if (sequence !== requestSequence.current) return;
      setLocks(result);
      setUnknownProfileIds(new Set());
      setRuntimeRunningProfileIds(new Set());
    } catch {
      if (sequence !== requestSequence.current) return;
      setLocks([]);
      setUnknownProfileIds(new Set());
      setRuntimeRunningProfileIds(new Set());
    }
  }, [localDeviceId, locallyRunningProfileIds, profileIds, workspaceId]);

  useEffect(() => {
    if (!BUGLOGIN_DESKTOP) return;
    let active = true;
    void invoke<string | null>("get_runtime_device_id")
      .then((deviceId) => {
        if (active) setLocalDeviceId(deviceId);
      })
      .catch(() => {
        if (active) setLocalDeviceId(null);
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    void fetchLocks();
    if (BUGLOGIN_DESKTOP) {
      const interval = window.setInterval(() => void fetchLocks(), 15_000);
      return () => window.clearInterval(interval);
    }

    const unlistenChanged = listen<{
      profileId: string | null;
      action: string;
    }>("profile-lock-changed", () => void fetchLocks());

    return () => {
      void unlistenChanged.then((fn) => {
        fn();
      });
    };
  }, [fetchLocks]);

  const isProfileLocked = useCallback(
    (profileId: string): boolean => {
      const lock = locks.find((l) => l.profileId === profileId);
      if (!lock) return false;
      if (BUGLOGIN_DESKTOP) return !locallyRunningProfileIds.has(profileId);
      if (currentUserId && lock.lockedBy === currentUserId) return false;
      return true;
    },
    [locks, currentUserId, locallyRunningProfileIds],
  );

  const getLockInfo = useCallback(
    (profileId: string): ProfileLockInfo | undefined => {
      return locks.find((l) => l.profileId === profileId);
    },
    [locks],
  );

  const isProfileLockUnknown = useCallback(
    (profileId: string) => BUGLOGIN_DESKTOP && unknownProfileIds.has(profileId),
    [unknownProfileIds],
  );

  return {
    locks,
    isProfileLocked,
    isProfileLockUnknown,
    runtimeRunningProfileIds,
    getLockInfo,
    refetchLocks: fetchLocks,
  };
}

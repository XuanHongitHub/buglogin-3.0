import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { resolveTeamLockState } from "@/lib/team-lock-state";
import type { ProfileLockInfo } from "@/types";

const BUGLOGIN_DESKTOP =
  process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop";
type CommandResult<T> = { data: T };
type LeaseStatus = {
  ownerId: string;
  issuedAtMs: number;
  expiresAtMs: number;
};
type RuntimeProjection = {
  deviceId: string;
  ownerId: string;
  ownerEmail?: string | null;
  state: "running" | "syncing" | "stopped";
  updatedAtMs: number;
} | null;
type RuntimeStatusSnapshot = {
  profileId: string;
  lease: LeaseStatus | null;
  runtime: RuntimeProjection;
};

export function useTeamLocks(
  currentUserId?: string,
  workspaceId?: string | null,
  profiles: Array<{ id: string }> = [],
  locallyRunningProfileIds: ReadonlySet<string> = new Set(),
) {
  const [locks, setLocks] = useState<ProfileLockInfo[]>([]);
  const [runtimeRunningProfileIds, setRuntimeRunningProfileIds] = useState<
    Set<string>
  >(new Set());
  const [localDeviceId, setLocalDeviceId] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const requestInProgress = useRef(false);
  const refreshQueued = useRef(false);
  const fetchLocksRef = useRef<
    (onlyProfileIds?: string[], queueIfBusy?: boolean) => Promise<void>
  >(async () => {});
  const profileIds = useMemo(
    () => profiles.map((profile) => profile.id),
    [profiles],
  );

  const executeFetchLocks = useCallback(
    async (onlyProfileIds?: string[]) => {
      const sequence = ++requestSequence.current;
      if (BUGLOGIN_DESKTOP) {
        if (!workspaceId || profileIds.length === 0) {
          setLocks([]);
          setRuntimeRunningProfileIds(new Set());
          return;
        }
        const nextLocks: ProfileLockInfo[] = [];
        const runtimeRunning = new Set<string>();
        const targetIds = onlyProfileIds
          ? profileIds.filter((profileId) => onlyProfileIds.includes(profileId))
          : profileIds;
        if (targetIds.length === 0) return;
        let statusReadSucceeded = false;
        try {
          const response = await invoke<CommandResult<RuntimeStatusSnapshot[]>>(
            "profile_runtime_status_batch",
            {
              correlationId: `runtime-status-batch-${crypto.randomUUID()}`,
              payload: { workspaceId, profileIds: targetIds },
            },
          );
          statusReadSucceeded = true;
          const snapshots = new Map(
            response.data.map((snapshot) => [snapshot.profileId, snapshot]),
          );
          for (const profileId of targetIds) {
            const snapshot = snapshots.get(profileId);
            if (!snapshot) {
              // No current lock was returned for this profile. Launch still
              // relies on the server's atomic lease acquisition.
              continue;
            }
            if (!snapshot.lease) continue;
            const lockState = resolveTeamLockState(
              snapshot.lease,
              snapshot.runtime,
              localDeviceId,
              locallyRunningProfileIds.has(profileId),
            );
            if (lockState.lockedRemotely) {
              if (lockState.runningRemotely) runtimeRunning.add(profileId);
              nextLocks.push({
                profileId,
                lockedBy: snapshot.lease.ownerId,
                lockedByEmail: snapshot.runtime?.ownerEmail ?? undefined,
                ownerLabel: snapshot.runtime?.ownerEmail ?? undefined,
                lockedAt: new Date(snapshot.lease.issuedAtMs).toISOString(),
                expiresAt: new Date(snapshot.lease.expiresAtMs).toISOString(),
                activity: lockState.runningRemotely
                  ? "running"
                  : lockState.syncingRemotely
                    ? "syncing"
                    : "locked",
              });
            }
          }
        } catch {
          // Keep the last confirmed lease for display. Start remains enabled
          // and the server performs the authoritative atomic acquire on click.
        }
        if (sequence !== requestSequence.current) return;
        if (!statusReadSucceeded) return;
        if (onlyProfileIds) {
          const refreshedIds = new Set(targetIds);
          setLocks((current) => [
            ...current.filter((lock) => !refreshedIds.has(lock.profileId)),
            ...nextLocks,
          ]);
          setRuntimeRunningProfileIds((current) => {
            const next = new Set(
              [...current].filter((profileId) => !refreshedIds.has(profileId)),
            );
            for (const profileId of runtimeRunning) next.add(profileId);
            return next;
          });
        } else {
          setLocks(nextLocks);
          setRuntimeRunningProfileIds(runtimeRunning);
        }
        return;
      }
      try {
        const result = await invoke<ProfileLockInfo[]>("get_team_locks");
        if (sequence !== requestSequence.current) return;
        setLocks(result);
        setRuntimeRunningProfileIds(new Set());
      } catch {
        if (sequence !== requestSequence.current) return;
        setLocks([]);
        setRuntimeRunningProfileIds(new Set());
      }
    },
    [localDeviceId, locallyRunningProfileIds, profileIds, workspaceId],
  );

  const fetchLocks = useCallback(
    async (onlyProfileIds?: string[], queueIfBusy = false) => {
      if (requestInProgress.current) {
        if (queueIfBusy) refreshQueued.current = true;
        return;
      }
      requestInProgress.current = true;
      try {
        await executeFetchLocks(onlyProfileIds);
      } finally {
        requestInProgress.current = false;
        if (refreshQueued.current) {
          refreshQueued.current = false;
          window.setTimeout(() => void fetchLocksRef.current(), 0);
        }
      }
    },
    [executeFetchLocks],
  );

  useEffect(() => {
    fetchLocksRef.current = fetchLocks;
  }, [fetchLocks]);

  const refetchLocks = useCallback(
    () => fetchLocks(undefined, true),
    [fetchLocks],
  );

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
      const interval = window.setInterval(() => {
        if (document.visibilityState === "visible") void fetchLocks();
      }, 15_000);
      const refreshWhenActive = () => {
        if (document.visibilityState === "visible") {
          void fetchLocks(undefined, true);
        }
      };
      window.addEventListener("focus", refreshWhenActive);
      window.addEventListener("online", refreshWhenActive);
      document.addEventListener("visibilitychange", refreshWhenActive);
      return () => {
        window.clearInterval(interval);
        window.removeEventListener("focus", refreshWhenActive);
        window.removeEventListener("online", refreshWhenActive);
        document.removeEventListener("visibilitychange", refreshWhenActive);
      };
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

  return {
    locks,
    isProfileLocked,
    runtimeRunningProfileIds,
    getLockInfo,
    refetchLocks,
  };
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { resolveTeamLockState } from "@/lib/team-lock-state";
import type { ProfileLockInfo } from "@/types";

const BUGLOGIN_DESKTOP =
  process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop";
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
          setUnknownProfileIds(new Set());
          setRuntimeRunningProfileIds(new Set());
          return;
        }
        const nextLocks: ProfileLockInfo[] = [];
        const checked = new Set<string>();
        const unconfirmed = new Set<string>();
        const runtimeRunning = new Set<string>();
        const targetIds = onlyProfileIds
          ? profileIds.filter((profileId) => onlyProfileIds.includes(profileId))
          : profileIds;
        if (targetIds.length === 0) return;
        await Promise.all(
          targetIds.map(async (profileId) => {
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
              const leaseResult = await invoke<
                CommandResult<LeaseStatus | null>
              >("resource_lease_get", {
                correlationId: `lease-status-${crypto.randomUUID()}`,
                payload: {
                  workspaceId: binding.workspaceId,
                  resourceId: binding.resourceId,
                },
              });
              const lease = leaseResult.data;
              checked.add(profileId);
              if (!lease) return;

              let runtime: RuntimeProjection;
              try {
                const runtimeResult = await invoke<
                  CommandResult<RuntimeProjection>
                >("profile_runtime_get", {
                  correlationId: `runtime-status-${crypto.randomUUID()}`,
                  payload: { profileId },
                });
                runtime = runtimeResult.data;
              } catch {
                unconfirmed.add(profileId);
                return;
              }

              const lockState = resolveTeamLockState(
                lease,
                runtime,
                localDeviceId,
                locallyRunningProfileIds.has(profileId),
              );
              if (lockState.unknown) {
                unconfirmed.add(profileId);
                return;
              }
              if (lockState.runningRemotely && runtime) {
                runtimeRunning.add(profileId);
                nextLocks.push({
                  profileId,
                  lockedBy: lease.ownerId,
                  lockedByEmail: runtime.ownerEmail ?? undefined,
                  ownerLabel: runtime.ownerEmail ?? undefined,
                  lockedAt: new Date(lease.issuedAtMs).toISOString(),
                  expiresAt: new Date(lease.expiresAtMs).toISOString(),
                });
              }
            } catch {
              // A failed read is unknown, not evidence of an active lock.
            }
          }),
        );
        // The server is authoritative; a failed read must not leave stale locks visible.
        if (sequence !== requestSequence.current) return;
        if (onlyProfileIds) {
          const refreshedIds = new Set(targetIds);
          setLocks((current) => [
            ...current.filter((lock) => !refreshedIds.has(lock.profileId)),
            ...nextLocks,
          ]);
          setUnknownProfileIds((current) => {
            const next = new Set(
              [...current].filter((profileId) => !refreshedIds.has(profileId)),
            );
            for (const profileId of targetIds) {
              if (!checked.has(profileId) || unconfirmed.has(profileId)) {
                next.add(profileId);
              }
            }
            return next;
          });
          setRuntimeRunningProfileIds((current) => {
            const next = new Set(
              [...current].filter((profileId) => !refreshedIds.has(profileId)),
            );
            for (const profileId of runtimeRunning) next.add(profileId);
            return next;
          });
        } else {
          setLocks(nextLocks);
          setUnknownProfileIds(
            new Set([
              ...profileIds.filter((profileId) => !checked.has(profileId)),
              ...unconfirmed,
            ]),
          );
          setRuntimeRunningProfileIds(runtimeRunning);
        }
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
      const interval = window.setInterval(() => void fetchLocks(), 15_000);
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

  useEffect(() => {
    if (!BUGLOGIN_DESKTOP || unknownProfileIds.size === 0) return;
    const interval = window.setInterval(
      () => void fetchLocks([...unknownProfileIds]),
      2_500,
    );
    return () => window.clearInterval(interval);
  }, [fetchLocks, unknownProfileIds]);

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
    refetchLocks,
  };
}

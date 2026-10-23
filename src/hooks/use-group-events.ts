import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import i18n from "@/i18n";
import type { GroupWithCount } from "@/types";

/**
 * Custom hook to manage group-related state and listen for backend events.
 * This hook eliminates the need for manual UI refreshes by automatically
 * updating state when the backend emits group change events.
 */
export function useGroupEvents() {
  const [groups, setGroups] = useState<GroupWithCount[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const groupsRequestRef = useRef(0);

  // Load groups from backend
  const loadGroups = useCallback(async () => {
    const request = ++groupsRequestRef.current;
    try {
      const groupsWithCounts = await invoke<GroupWithCount[]>(
        "get_groups_with_profile_counts",
      );
      if (request !== groupsRequestRef.current) return;
      setGroups(groupsWithCounts);
      setError(null);
    } catch (err: unknown) {
      if (request !== groupsRequestRef.current) return;
      console.warn("Passive load groups notice:", err);
      const isUnavailable =
        typeof err === "object" &&
        err !== null &&
        ((err as any).code === "api_unavailable" ||
          (err as any).status === 503);
      if (!isUnavailable) {
        const errorDetail =
          typeof err === "object" && err !== null && "title" in err
            ? String((err as any).title)
            : String(err);
        setError(i18n.t("errors.loadGroupsFailed", { error: errorDetail }));
      }
    }
  }, []);

  // Clear error state
  const clearError = useCallback(() => {
    setError(null);
  }, []);

  // Initial load and event listeners setup
  useEffect(() => {
    let disposed = false;
    let groupsUnlisten: (() => void) | undefined;
    let profilesUnlisten: (() => void) | undefined;
    let listenerError: string | undefined;

    const registerListener = async (
      name: string,
      subscribe: () => Promise<() => void>,
      assign: (unlisten: () => void) => void,
    ) => {
      if (disposed) return;
      try {
        const unlisten = await subscribe();
        if (disposed) unlisten();
        else assign(unlisten);
      } catch (err) {
        console.error(`Failed to register ${name} listener:`, err);
        if (!disposed) {
          listenerError = i18n.t("errors.setupGroupListenersFailed", {
            error: JSON.stringify(err),
          });
          setError(listenerError);
        }
      }
    };

    const setupListeners = async () => {
      try {
        await registerListener(
          "groups-changed",
          () =>
            listen("groups-changed", () => {
              console.log("Received groups-changed event, reloading groups");
              void loadGroups();
            }),
          (unlisten) => (groupsUnlisten = unlisten),
        );

        // Also listen for profile changes since groups show profile counts
        await registerListener(
          "profiles-changed",
          () =>
            listen("profiles-changed", () => {
              console.log(
                "Received profiles-changed event, reloading groups for updated counts",
              );
              void loadGroups();
            }),
          (unlisten) => (profilesUnlisten = unlisten),
        );

        // Subscribe before the initial request so workspace refresh events are
        // not missed while that request is pending.
        if (disposed) return;
        await loadGroups();

        console.log("Group event listeners set up successfully");
      } catch (err) {
        console.error("Failed to setup group event listeners:", err);
        if (!disposed) {
          setError(
            i18n.t("errors.setupGroupListenersFailed", {
              error: JSON.stringify(err),
            }),
          );
        }
      } finally {
        if (!disposed) {
          if (listenerError) setError(listenerError);
          setIsLoading(false);
        }
      }
    };

    void setupListeners();

    // Cleanup listeners on unmount. The two unlisten fns stay in separate
    // locals: merging them into one wrapper that reassigns the local it then
    // calls makes that wrapper call itself, and the stack overflow aborts the
    // teardown mid-commit, leaving stale overlay nodes that swallow clicks.
    return () => {
      disposed = true;
      groupsRequestRef.current += 1;
      if (groupsUnlisten) groupsUnlisten();
      if (profilesUnlisten) profilesUnlisten();
    };
  }, [loadGroups]);

  return {
    groups,
    isLoading,
    error,
    loadGroups,
    clearError,
  };
}

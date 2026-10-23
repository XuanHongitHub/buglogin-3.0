import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import i18n from "@/i18n";
import type { BrowserProfile, GroupWithCount } from "@/types";

interface UseProfileEventsReturn {
  profiles: BrowserProfile[];
  groups: GroupWithCount[];
  runningProfiles: Set<string>;
  isLoading: boolean;
  error: string | null;
  loadProfiles: () => Promise<void>;
  loadGroups: () => Promise<void>;
  clearError: () => void;
}

/**
 * Custom hook to manage profile-related state and listen for backend events.
 * This hook eliminates the need for manual UI refreshes by automatically
 * updating state when the backend emits profile change events.
 */
export function useProfileEvents(): UseProfileEventsReturn {
  const [profiles, setProfiles] = useState<BrowserProfile[]>([]);
  const [groups, setGroups] = useState<GroupWithCount[]>([]);
  const [runningProfiles, setRunningProfiles] = useState<Set<string>>(
    new Set(),
  );
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const profilesRequestRef = useRef(0);
  const groupsRequestRef = useRef(0);
  const runningRequestRef = useRef(0);
  const runningStateRevisionRef = useRef(0);

  // Load profiles from backend
  const loadProfiles = useCallback(async () => {
    const request = ++profilesRequestRef.current;
    try {
      const profileList = await invoke<BrowserProfile[]>(
        "list_browser_profiles",
      );
      if (request !== profilesRequestRef.current) return;
      setProfiles(profileList);
      setError(null);
    } catch (err: unknown) {
      if (request !== profilesRequestRef.current) return;
      console.warn("Passive load profiles notice:", err);
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
        setError(i18n.t("errors.loadProfilesFailed", { error: errorDetail }));
      }
    }
  }, []);

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
    } catch (err) {
      if (request !== groupsRequestRef.current) return;
      console.error("Failed to load groups with counts:", err);
      setGroups([]);
    }
  }, []);

  // Clear error state
  const clearError = useCallback(() => {
    setError(null);
  }, []);

  // Initial load and event listeners setup
  useEffect(() => {
    let disposed = false;
    let profilesUnlisten: (() => void) | undefined;
    let runningUnlisten: (() => void) | undefined;
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
          listenerError = i18n.t("errors.setupProfileListenersFailed", {
            error: JSON.stringify(err),
          });
          setError(listenerError);
        }
      }
    };

    const setupListeners = async () => {
      try {
        // Listen for profile changes (create, delete, rename, update, etc.)
        await registerListener(
          "profiles-changed",
          () =>
            listen("profiles-changed", () => {
              console.log(
                "Received profiles-changed event, reloading profiles and groups",
              );
              void loadProfiles();
              void loadGroups();
            }),
          (unlisten) => (profilesUnlisten = unlisten),
        );

        // Listen for profile running state changes
        await registerListener(
          "profile-running-changed",
          () =>
            listen<{ id: string; is_running: boolean }>(
              "profile-running-changed",
              (event) => {
                const { id, is_running } = event.payload;
                runningStateRevisionRef.current += 1;
                setRunningProfiles((prev) => {
                  const next = new Set(prev);
                  if (is_running) {
                    next.add(id);
                  } else {
                    next.delete(id);
                  }
                  return next;
                });
              },
            ),
          (unlisten) => (runningUnlisten = unlisten),
        );

        // Subscribe before the initial requests. Workspace changes can happen
        // while those requests are pending; request generations prevent stale
        // responses from replacing data loaded after the change.
        if (disposed) return;
        await Promise.all([loadProfiles(), loadGroups()]);

        console.log("Profile event listeners set up successfully");
      } catch (err) {
        console.error("Failed to setup profile event listeners:", err);
        if (!disposed) {
          setError(
            i18n.t("errors.setupProfileListenersFailed", {
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

    // Cleanup listeners on unmount, including requests still in flight.
    return () => {
      disposed = true;
      profilesRequestRef.current += 1;
      groupsRequestRef.current += 1;
      if (profilesUnlisten) profilesUnlisten();
      if (runningUnlisten) runningUnlisten();
    };
  }, [loadProfiles, loadGroups]);

  // Persisted process_id/profile state can outlive the browser process. Ask
  // the runtime owner for its live process set instead of treating database
  // state as proof that a browser is still open.
  useEffect(() => {
    let active = true;
    let requestInFlight = false;
    const reconcile = () => {
      if (requestInFlight) return;
      requestInFlight = true;
      const request = ++runningRequestRef.current;
      const stateRevision = runningStateRevisionRef.current;
      void invoke<string[]>("list_running_browser_profiles")
        .then((ids) => {
          if (
            active &&
            request === runningRequestRef.current &&
            stateRevision === runningStateRevisionRef.current
          ) {
            setRunningProfiles(new Set(ids));
          }
        })
        .catch((err: unknown) => {
          console.error("Failed to reconcile running browser processes:", err);
        })
        .finally(() => {
          requestInFlight = false;
        });
    };
    reconcile();
    const timer = window.setInterval(reconcile, 2_000);
    return () => {
      active = false;
      runningRequestRef.current += 1;
      window.clearInterval(timer);
    };
  }, []);

  return {
    profiles,
    groups,
    runningProfiles,
    isLoading,
    error,
    loadProfiles,
    loadGroups,
    clearError,
  };
}

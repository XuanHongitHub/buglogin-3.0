import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import i18n from "@/i18n";
import { followLatestProfileLoadResult } from "@/lib/profile-workspace-state";
import type { BrowserProfile } from "@/types";

interface UseProfileEventsReturn {
  profiles: BrowserProfile[];
  runningProfiles: Set<string>;
  isLoading: boolean;
  isRefreshing: boolean;
  error: string | null;
  loadProfiles: (workspaceId?: string) => Promise<boolean>;
  clearError: () => void;
}

/**
 * Custom hook to manage profile-related state and listen for backend events.
 * This hook eliminates the need for manual UI refreshes by automatically
 * updating state when the backend emits profile change events.
 */
export function useProfileEvents({
  skipInitialLoad = false,
  workspaceId,
}: {
  skipInitialLoad?: boolean;
  workspaceId?: string;
} = {}): UseProfileEventsReturn {
  const [profiles, setProfiles] = useState<BrowserProfile[]>([]);
  const [runningProfiles, setRunningProfiles] = useState<Set<string>>(
    new Set(),
  );
  const [isLoading, setIsLoading] = useState(true);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const profilesRequestRef = useRef(0);
  const profilesRequestPromiseRef = useRef<Promise<boolean> | null>(null);
  const activeWorkspaceIdRef = useRef<string | undefined>(workspaceId);
  activeWorkspaceIdRef.current = workspaceId;
  const runningRequestRef = useRef(0);
  const runningStateRevisionRef = useRef(0);

  // Load profiles from backend
  const loadProfiles = useCallback(
    (requestedWorkspaceId?: string): Promise<boolean> => {
      const request = ++profilesRequestRef.current;
      const activeWorkspaceId =
        requestedWorkspaceId ?? activeWorkspaceIdRef.current;
      setIsRefreshing(true);
      const operation = (async (): Promise<boolean | null> => {
        try {
          const profileList = await invoke<BrowserProfile[]>(
            "list_browser_profiles",
            activeWorkspaceId ? { workspaceId: activeWorkspaceId } : undefined,
          );
          if (request !== profilesRequestRef.current) return null;
          setProfiles(profileList);
          setError(null);
          return true;
        } catch (err: unknown) {
          if (request !== profilesRequestRef.current) return null;
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
            setError(
              i18n.t("errors.loadProfilesFailed", { error: errorDetail }),
            );
          }
          return false;
        } finally {
          if (request === profilesRequestRef.current) setIsRefreshing(false);
        }
      })();
      const result: Promise<boolean> = operation.then(async (success) => {
        return followLatestProfileLoadResult(
          success,
          profilesRequestPromiseRef.current,
          result,
        );
      });
      profilesRequestPromiseRef.current = result;
      return result;
    },
    [],
  );

  // Clear error state
  const clearError = useCallback(() => {
    setError(null);
  }, []);

  // Initial load and event listeners setup
  useEffect(() => {
    let disposed = false;
    let profilesUnlisten: (() => void) | undefined;
    let profileUpdatedUnlisten: (() => void) | undefined;
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
                "Received profiles-changed event, reloading profiles",
              );
              void loadProfiles();
            }),
          (unlisten) => (profilesUnlisten = unlisten),
        );
        // Several profile edit screens publish this event after saving. Keep
        // the central list in sync with both event names used by the desktop.
        await registerListener(
          "profile-updated",
          () =>
            listen("profile-updated", () => {
              void loadProfiles();
            }),
          (unlisten) => (profileUpdatedUnlisten = unlisten),
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
        if (!skipInitialLoad) await loadProfiles();

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
      if (profilesUnlisten) profilesUnlisten();
      if (profileUpdatedUnlisten) profileUpdatedUnlisten();
      if (runningUnlisten) runningUnlisten();
    };
  }, [loadProfiles, skipInitialLoad]);

  // Recover from missed desktop events and refresh when returning to the app.
  useEffect(() => {
    const refreshWhenVisible = () => {
      if (document.visibilityState === "visible") void loadProfiles();
    };
    const timer = window.setInterval(() => void loadProfiles(), 30_000);
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [loadProfiles]);

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
    runningProfiles,
    isLoading,
    isRefreshing,
    error,
    loadProfiles,
    clearError,
  };
}

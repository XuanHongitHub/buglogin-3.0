import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import i18n from "@/i18n";
import type { StoredProxy } from "@/types";

/**
 * Custom hook to manage proxy-related state and listen for backend events.
 * This hook eliminates the need for manual UI refreshes by automatically
 * updating state when the backend emits proxy change events.
 */
export function useProxyEvents() {
  const [storedProxies, setStoredProxies] = useState<StoredProxy[]>([]);
  const [proxyUsage, setProxyUsage] = useState<Record<string, number>>({});
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const proxiesRequestRef = useRef(0);
  const usageRequestRef = useRef(0);

  // Load proxy usage (how many profiles are using each proxy)
  const loadProxyUsage = useCallback(async () => {
    const request = ++usageRequestRef.current;
    try {
      const profiles = await invoke<{ proxy_id?: string }[]>(
        "list_browser_profiles",
      );
      if (request !== usageRequestRef.current) return;
      const counts: Record<string, number> = {};
      for (const p of profiles) {
        if (p.proxy_id) counts[p.proxy_id] = (counts[p.proxy_id] ?? 0) + 1;
      }
      setProxyUsage(counts);
    } catch (err) {
      if (request === usageRequestRef.current) {
        console.error("Failed to load proxy usage:", err);
      }
    }
  }, []);

  // Load proxies from backend
  const loadProxies = useCallback(async () => {
    const request = ++proxiesRequestRef.current;
    try {
      const stored = await invoke<StoredProxy[]>("get_stored_proxies");
      if (request !== proxiesRequestRef.current) return;
      setStoredProxies(stored);
      await loadProxyUsage();
      if (request !== proxiesRequestRef.current) return;
      setError(null);
    } catch (err: unknown) {
      if (request !== proxiesRequestRef.current) return;
      console.warn("Passive load proxies notice:", err);
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
        setError(i18n.t("errors.loadProxiesFailed", { error: errorDetail }));
      }
    }
  }, [loadProxyUsage]);

  // Clear error state
  const clearError = useCallback(() => {
    setError(null);
  }, []);

  // Initial load and event listeners setup
  useEffect(() => {
    let disposed = false;
    let proxiesUnlisten: (() => void) | undefined;
    let profilesUnlisten: (() => void) | undefined;
    let storedProxiesUnlisten: (() => void) | undefined;
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
          listenerError = i18n.t("errors.setupProxyListenersFailed", {
            error: JSON.stringify(err),
          });
          setError(listenerError);
        }
      }
    };

    const setupListeners = async () => {
      try {
        // Listen for proxy changes (create, delete, update, start, stop, etc.)
        await registerListener(
          "proxies-changed",
          () =>
            listen("proxies-changed", () => {
              console.log("Received proxies-changed event, reloading proxies");
              void loadProxies();
            }),
          (unlisten) => (proxiesUnlisten = unlisten),
        );

        // Listen for profile changes to update proxy usage counts
        await registerListener(
          "profiles-changed",
          () =>
            listen("profiles-changed", () => {
              console.log(
                "Received profiles-changed event, reloading proxy usage",
              );
              void loadProxyUsage();
            }),
          (unlisten) => (profilesUnlisten = unlisten),
        );

        // Listen for profile updates to update proxy usage counts
        await registerListener(
          "stored-proxies-changed",
          () =>
            listen("stored-proxies-changed", () => {
              console.log(
                "Received stored-proxies-changed event, reloading proxies",
              );
              void loadProxies();
            }),
          (unlisten) => (storedProxiesUnlisten = unlisten),
        );

        // Subscribe before the initial requests so workspace refresh events
        // cannot be lost while proxy and profile data is loading.
        if (disposed) return;
        await loadProxies();

        console.log("Proxy event listeners set up successfully");
      } catch (err) {
        console.error("Failed to setup proxy event listeners:", err);
        if (!disposed) {
          setError(
            i18n.t("errors.setupProxyListenersFailed", {
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

    // Cleanup listeners on unmount
    return () => {
      disposed = true;
      proxiesRequestRef.current += 1;
      usageRequestRef.current += 1;
      if (proxiesUnlisten) proxiesUnlisten();
      if (profilesUnlisten) profilesUnlisten();
      if (storedProxiesUnlisten) storedProxiesUnlisten();
    };
  }, [loadProxies, loadProxyUsage]);

  return {
    storedProxies,
    proxyUsage,
    isLoading,
    error,
    loadProxies,
    clearError,
  };
}

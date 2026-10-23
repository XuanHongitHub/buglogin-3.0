import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { CloudAuthState, CloudUser } from "@/types";
import {
  createTauriDesktopApi,
  DESKTOP_AUTH_STATUS_CHANGED_EVENT,
} from "../../apps/desktop/src/bridge";
import type { AccountView } from "../../frontend/src/generated/command-registry";

const BUGLOGIN_DESKTOP =
  process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop";
const desktopApi = BUGLOGIN_DESKTOP ? createTauriDesktopApi() : null;

interface UseCloudAuthReturn {
  user: CloudUser | null;
  account: AccountView | null;
  isLoggedIn: boolean;
  isLoading: boolean;
  exchangeDeviceCode: (code: string) => Promise<CloudAuthState | null>;
  logout: () => Promise<void>;
  refreshProfile: () => Promise<CloudUser | null>;
}

export function useCloudAuth(): UseCloudAuthReturn {
  const [authState, setAuthState] = useState<CloudAuthState | null>(null);
  const [account, setAccount] = useState<AccountView | null>(null);
  const [desktopAuthenticated, setDesktopAuthenticated] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  const loadUser = useCallback(async () => {
    try {
      if (desktopApi) {
        const status = await desktopApi.authStatus();
        const authenticated = status.state === "authenticated";
        setDesktopAuthenticated(authenticated);
        if (authenticated) {
          setAccount(await desktopApi.getAccount());
        } else {
          setAccount(null);
        }
        setAuthState(null);
        return;
      }
      const state = await invoke<CloudAuthState | null>("cloud_get_user");
      setAuthState(state);
    } catch (error) {
      console.error("Failed to load cloud auth state:", error);
      setAuthState(null);
      setAccount(null);
      setDesktopAuthenticated(false);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadUser();
    if (desktopApi) return;

    const unlistenExpired = listen("cloud-auth-expired", () => {
      setAuthState(null);
    });

    const unlistenChanged = listen("cloud-auth-changed", () => {
      void loadUser();
    });

    return () => {
      void unlistenExpired.then((unlisten) => {
        unlisten();
      });
      void unlistenChanged.then((unlisten) => {
        unlisten();
      });
    };
  }, [loadUser]);

  const exchangeDeviceCode = useCallback(
    async (code: string): Promise<CloudAuthState | null> => {
      if (desktopApi) {
        // BugLogin desktop authorization is started and polled by AuthBootstrap.
        // A pasted Donut link code is not a BugLogin credential.
        throw new Error(
          "Sign in through the BugLogin device authorization screen.",
        );
      }
      const state = await invoke<CloudAuthState>("cloud_exchange_device_code", {
        code,
      });
      setAuthState(state);
      return state;
    },
    [],
  );

  const logout = useCallback(async () => {
    if (desktopApi) {
      await desktopApi.logout();
      setAccount(null);
      setDesktopAuthenticated(false);
      setAuthState(null);
      window.dispatchEvent(new Event(DESKTOP_AUTH_STATUS_CHANGED_EVENT));
      return;
    }
    await invoke("cloud_logout");
    setAuthState(null);
  }, []);

  const refreshProfile = useCallback(async (): Promise<CloudUser | null> => {
    if (desktopApi) {
      const status = await desktopApi.authStatus();
      const authenticated = status.state === "authenticated";
      setDesktopAuthenticated(authenticated);
      const current = authenticated ? await desktopApi.getAccount() : null;
      setAccount(current);
      return null;
    }
    const user = await invoke<CloudUser>("cloud_refresh_profile");
    setAuthState((prev) =>
      prev
        ? { ...prev, user }
        : { user, logged_in_at: new Date().toISOString() },
    );
    return user;
  }, []);

  return {
    user: authState?.user ?? null,
    account,
    isLoggedIn: desktopApi ? desktopAuthenticated : authState !== null,
    isLoading,
    exchangeDeviceCode,
    logout,
    refreshProfile,
  };
}

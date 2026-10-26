"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { AppUpdateToast } from "@/components/app-update-toast";
import { translateBackendError } from "@/lib/backend-errors";
import { showToast } from "@/lib/toast-utils";
import type { AppUpdateInfo, AppUpdateProgress } from "@/types";
import { shouldPrepareAppUpdate } from "./app-update-lifecycle";

function isUpdaterNotConfigured(error: unknown): boolean {
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : error && typeof error === "object" && "code" in error
          ? String(error.code)
          : "";
  return message.includes("app_updater_not_configured");
}

export function useAppUpdateNotifications() {
  const { t } = useTranslation();
  const [updateInfo, setUpdateInfo] = useState<AppUpdateInfo | null>(null);
  const [isUpdating, setIsUpdating] = useState(false);
  const [isCheckingForUpdates, setIsCheckingForUpdates] = useState(false);
  const [updateProgress, setUpdateProgress] =
    useState<AppUpdateProgress | null>(null);
  const [updateReady, setUpdateReady] = useState(false);
  const [manualUpdatePrompt, setManualUpdatePrompt] = useState(false);
  const [isClient, setIsClient] = useState(false);
  const [autoUpdatePreferenceLoaded, setAutoUpdatePreferenceLoaded] =
    useState(false);
  const [autoUpdatesEnabled, setAutoUpdatesEnabled] = useState(false);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);
  const autoDownloadedVersion = useRef<string | null>(null);
  const updateCheckRef = useRef<Promise<AppUpdateInfo | null> | null>(null);
  const manualCheckInFlightRef = useRef(false);
  const manualCheckGenerationRef = useRef(0);
  const backgroundUpdateDuringManualCheckRef = useRef<AppUpdateInfo | null>(
    null,
  );

  const requestAppUpdateCheck = useCallback(() => {
    if (updateCheckRef.current) return updateCheckRef.current;
    const request = invoke<AppUpdateInfo | null>(
      "check_for_app_updates_manual",
    );
    const pending = request.finally(() => {
      if (updateCheckRef.current === pending) updateCheckRef.current = null;
    });
    updateCheckRef.current = pending;
    return pending;
  }, []);

  // Ensure we're on the client side to prevent hydration mismatches
  useEffect(() => {
    setIsClient(true);
  }, []);

  useEffect(() => {
    if (!isClient) return;
    void invoke<{ disableAutoUpdates?: boolean }>("get_app_settings")
      .then((settings) => {
        setAutoUpdatesEnabled(settings.disableAutoUpdates !== true);
      })
      .catch((error) => {
        console.error("Failed to read automatic update preference:", error);
        setAutoUpdatesEnabled(false);
      })
      .finally(() => {
        setAutoUpdatePreferenceLoaded(true);
      });
  }, [isClient]);

  const checkForAppUpdates = useCallback(async () => {
    if (!isClient || !autoUpdatePreferenceLoaded || !autoUpdatesEnabled) return;

    const manualGeneration = manualCheckGenerationRef.current;
    try {
      setIsCheckingForUpdates(true);
      const update = await requestAppUpdateCheck();

      if (
        manualCheckInFlightRef.current ||
        manualCheckGenerationRef.current !== manualGeneration
      ) {
        if (update) backgroundUpdateDuringManualCheckRef.current = update;
        return;
      }

      // Don't show update if this version was already dismissed
      if (update && update.new_version !== dismissedVersion) {
        setManualUpdatePrompt(false);
        setUpdateInfo(update);
      } else if (update) {
        console.log("Update available but dismissed:", update.new_version);
      } else {
        setManualUpdatePrompt(false);
        setUpdateInfo(null);
      }
    } catch (error) {
      if (!isUpdaterNotConfigured(error)) {
        console.error("Failed to check for app updates:", error);
      }
    } finally {
      setIsCheckingForUpdates(false);
    }
  }, [
    isClient,
    autoUpdatePreferenceLoaded,
    autoUpdatesEnabled,
    dismissedVersion,
    requestAppUpdateCheck,
  ]);

  const checkForAppUpdatesManual = useCallback(async () => {
    if (!isClient) return;

    manualCheckGenerationRef.current += 1;
    manualCheckInFlightRef.current = true;
    backgroundUpdateDuringManualCheckRef.current = null;
    try {
      console.log("Triggering manual app update check...");
      setIsCheckingForUpdates(true);
      const checkedUpdate = await requestAppUpdateCheck();
      const update =
        backgroundUpdateDuringManualCheckRef.current ?? checkedUpdate;
      console.log("Manual check result:", update);

      // Always show manual check results, even if previously dismissed
      autoDownloadedVersion.current = null;
      setManualUpdatePrompt(Boolean(update));
      setUpdateInfo(update);
      if (!update) {
        showToast({ type: "success", title: t("about.upToDate") });
      }
    } catch (error) {
      if (isUpdaterNotConfigured(error)) {
        showToast({
          type: "info",
          title: t("about.updatesReleaseOnly"),
          description: t("about.updatesReleaseOnlyDescription"),
        });
        return;
      }
      console.error("Failed to manually check for app updates:", error);
      showToast({
        type: "error",
        title: t("about.updateCheckFailed"),
        description: translateBackendError(t, error),
        duration: 6000,
      });
    } finally {
      manualCheckInFlightRef.current = false;
      backgroundUpdateDuringManualCheckRef.current = null;
      setIsCheckingForUpdates(false);
    }
  }, [isClient, requestAppUpdateCheck, t]);

  const handleAppUpdate = useCallback(
    async (appUpdateInfo: AppUpdateInfo) => {
      try {
        setIsUpdating(true);
        setUpdateProgress({
          stage: "downloading",
          percentage: 0,
          speed: undefined,
          eta: undefined,
          message: t("appUpdate.toast.startingUpdate"),
        });

        await invoke("download_and_prepare_app_update", {
          updateInfo: appUpdateInfo,
        });
      } catch (error) {
        console.error("Failed to update app:", error);
        showToast({
          type: "error",
          title: t("appUpdate.toast.updateFailed"),
          description: translateBackendError(t, error),
          duration: 6000,
        });
        setIsUpdating(false);
        setUpdateProgress(null);
        // Deliberately NOT resetting autoDownloadedVersion here: the
        // auto-download effect re-runs as soon as isUpdating flips back to
        // false, so clearing the marker now would retry in a tight loop.
        // Retries are re-armed when the next backend check delivers a fresh
        // update event instead.
      }
    },
    [t],
  );

  const handleRestart = useCallback(async () => {
    try {
      await invoke("restart_application");
    } catch (error) {
      console.error("Failed to restart app:", error);
      showToast({
        type: "error",
        title: t("appUpdate.toast.restartFailed"),
        description: translateBackendError(t, error),
        duration: 6000,
      });
    }
  }, [t]);

  const handleManualUpdate = useCallback(() => {
    if (
      !updateInfo ||
      !shouldPrepareAppUpdate({
        source: "manual",
        updateAvailable: Boolean(updateInfo),
        consented: true,
        autoUpdatesEnabled,
        isBusy: isUpdating,
        updateReady,
      })
    )
      return;
    autoDownloadedVersion.current = updateInfo.new_version;
    setManualUpdatePrompt(false);
    void handleAppUpdate(updateInfo);
  }, [
    autoUpdatesEnabled,
    handleAppUpdate,
    isUpdating,
    updateInfo,
    updateReady,
  ]);

  const dismissAppUpdate = useCallback(() => {
    if (!isClient) return;

    if (updateReady) {
      void invoke("discard_prepared_app_update").catch((error) => {
        console.error("Failed to discard prepared app update:", error);
      });
    }

    // Remember the dismissed version so we don't show it again
    if (updateInfo) {
      setDismissedVersion(updateInfo.new_version);
      console.log("Dismissed app update version:", updateInfo.new_version);
    }

    setUpdateInfo(null);
    setManualUpdatePrompt(false);
    setUpdateReady(false);
    setIsUpdating(false);
    setUpdateProgress(null);
    toast.dismiss("app-update");
  }, [isClient, updateInfo, updateReady]);

  // Listen for app update events
  useEffect(() => {
    if (!isClient) return;

    const unlistenUpdate = listen<AppUpdateInfo>(
      "app-update-available",
      (event) => {
        console.log("App update available:", event.payload);
        if (manualCheckInFlightRef.current) {
          backgroundUpdateDuringManualCheckRef.current = event.payload;
          return;
        }
        // A fresh backend check re-arms auto-download, so a version whose
        // earlier attempt failed (e.g. a transient checksum-fetch error) is
        // retried once per periodic check instead of staying blocked until
        // restart. The effect's updateReady guard keeps an already-prepared
        // update from being downloaded again.
        autoDownloadedVersion.current = null;
        setManualUpdatePrompt(false);
        setUpdateInfo(event.payload);
      },
    );

    const unlistenProgress = listen<AppUpdateProgress>(
      "app-update-progress",
      (event) => {
        setUpdateProgress(event.payload);
      },
    );

    const unlistenReady = listen<string>("app-update-ready", (event) => {
      console.log("App update ready:", event.payload);
      setUpdateReady(true);
      setIsUpdating(false);
      setUpdateProgress(null);
    });

    return () => {
      void unlistenUpdate.then((unlisten) => {
        unlisten();
      });
      void unlistenProgress.then((unlisten) => {
        unlisten();
      });
      void unlistenReady.then((unlisten) => {
        unlisten();
      });
    };
  }, [isClient]);

  // Background checks may download automatically. Manual checks are check-only
  // until the user selects Update now in the toast.
  useEffect(() => {
    if (!updateInfo || updateInfo.manual_update_required || manualUpdatePrompt)
      return;
    if (
      !shouldPrepareAppUpdate({
        source: "background",
        updateAvailable: Boolean(updateInfo),
        consented: false,
        autoUpdatesEnabled,
        isBusy: isUpdating,
        updateReady,
      }) ||
      !isClient ||
      autoDownloadedVersion.current === updateInfo.new_version
    )
      return;

    autoDownloadedVersion.current = updateInfo.new_version;
    console.log("Auto-downloading app update:", updateInfo.new_version);
    void handleAppUpdate(updateInfo);
  }, [
    isClient,
    autoUpdatesEnabled,
    updateInfo,
    manualUpdatePrompt,
    isUpdating,
    updateReady,
    handleAppUpdate,
  ]);

  const [bannerDismissed, setBannerDismissed] = useState(false);
  const [isUpdateDialogOpen, setIsUpdateDialogOpen] = useState(false);

  // Auto-reset banner dismissal when a fresh update becomes ready
  useEffect(() => {
    if (updateReady || manualUpdatePrompt) {
      setBannerDismissed(false);
    }
  }, [updateReady, manualUpdatePrompt]);

  const dismissBanner = useCallback(() => {
    setBannerDismissed(true);
  }, []);

  // Check for app updates on startup
  useEffect(() => {
    if (!isClient || !autoUpdatePreferenceLoaded || !autoUpdatesEnabled) return;

    // Check for updates immediately on startup
    void checkForAppUpdates();
  }, [
    isClient,
    autoUpdatePreferenceLoaded,
    autoUpdatesEnabled,
    checkForAppUpdates,
  ]);

  return {
    updateInfo,
    isUpdating,
    isCheckingForUpdates,
    updateProgress,
    updateReady,
    manualUpdatePrompt,
    bannerDismissed,
    isUpdateDialogOpen,
    setIsUpdateDialogOpen,
    dismissBanner,
    checkForAppUpdates,
    checkForAppUpdatesManual,
    handleAppUpdate: handleManualUpdate,
    handleRestart,
    dismissAppUpdate,
    autoUpdatesEnabled,
    setAutoUpdatesEnabled,
  };
}

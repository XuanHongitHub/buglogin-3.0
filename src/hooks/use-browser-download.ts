const inFlightDownloads = new Map<string, Promise<void>>();

import { invoke } from "@tauri-apps/api/core";
import type { Event as TauriEvent } from "@tauri-apps/api/event";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import i18n from "@/i18n";
import { translateBackendError } from "@/lib/backend-errors";
import { getBrowserDisplayName } from "@/lib/browser-utils";
import { isOnboardingActive } from "@/lib/onboarding-signal";
import {
  dismissToast,
  showDownloadToast,
  showErrorToast,
} from "@/lib/toast-utils";

interface GithubRelease {
  tag_name: string;
  assets: {
    name: string;
    browser_download_url: string;
    hash?: string;
  }[];
  published_at: string;
  is_nightly: boolean;
}

interface RuntimeCatalog {
  schemaVersion: number;
  releases: Array<{ runtime: string; version: string }>;
}

interface DownloadProgress {
  browser: string;
  version: string;
  downloaded_bytes: number;
  total_bytes?: number;
  percentage: number;
  speed_bytes_per_sec: number;
  eta_seconds?: number;
  stage: string;
  error?: string;
}

export function useBrowserDownload() {
  const [availableVersions, setAvailableVersions] = useState<GithubRelease[]>(
    [],
  );
  const [downloadedVersionsMap, setDownloadedVersionsMap] = useState<
    Record<string, string[]>
  >({});
  const [downloadingBrowsers, setDownloadingBrowsers] = useState<Set<string>>(
    new Set(),
  );
  const [downloadProgress, setDownloadProgress] =
    useState<DownloadProgress | null>(null);
  // Last non-terminal stage seen per browser-version. The backend's terminal
  // "error" event does not say which phase failed, and only a failed
  // extraction deletes the archive, so the phase it interrupted is what
  // decides which message the user gets.
  const lastDownloadStageRef = useRef(new Map<string, string>());

  const loadCatalogVersions = useCallback(async (browserStr: string) => {
    const catalog = await invoke<RuntimeCatalog>(
      "runtime_catalog_current_device_get",
      { correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}` },
    );
    return catalog.releases
      .filter((release) => release.runtime === browserStr)
      .map((release) => ({ version: release.version, date: "" }));
  }, []);

  const formatTime = useCallback((seconds: number): string => {
    if (seconds < 60) {
      return `${Math.round(seconds)}s`;
    }
    if (seconds < 3600) {
      const minutes = Math.floor(seconds / 60);
      const remainingSeconds = Math.round(seconds % 60);
      return `${minutes}m ${remainingSeconds}s`;
    }
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${minutes}m`;
  }, []);

  const formatBytes = useCallback((bytes: number): string => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${Number.parseFloat((bytes / k ** i).toFixed(1))} ${sizes[i]}`;
  }, []);

  const loadVersions = useCallback(
    async (browserStr: string) => {
      // Use a simple loading state instead of toast for version fetching
      console.log(`Fetching ${browserStr} versions...`);

      try {
        const versionInfos = await loadCatalogVersions(browserStr);

        // Convert BrowserVersionInfo to GithubRelease format for compatibility
        const githubReleases: GithubRelease[] = versionInfos.map(
          (versionInfo) => ({
            tag_name: versionInfo.version,
            assets: [],
            published_at: versionInfo.date,
            is_nightly: false,
          }),
        );

        setAvailableVersions(githubReleases);
        return githubReleases;
      } catch (error) {
        console.error("Failed to load versions:", error);
        throw error;
      }
    },
    [loadCatalogVersions],
  );

  const loadVersionsWithNewCount = useCallback(
    async (browserStr: string) => {
      try {
        // Get versions with new count info and cached detailed info
        const versionInfos = await loadCatalogVersions(browserStr);

        // Convert BrowserVersionInfo to GithubRelease format for compatibility
        const githubReleases: GithubRelease[] = versionInfos.map(
          (versionInfo) => ({
            tag_name: versionInfo.version,
            assets: [],
            published_at: versionInfo.date,
            is_nightly: false,
          }),
        );

        setAvailableVersions(githubReleases);

        // Show notification about new versions if any were found
        return githubReleases;
      } catch (error) {
        console.error("Failed to load versions:", error);
        throw error;
      }
    },
    [loadCatalogVersions],
  );

  const loadDownloadedVersions = useCallback(async (browserStr: string) => {
    try {
      const versions = await invoke<string[]>(
        "runtime_installed_versions_get",
        {
          runtime: browserStr,
          correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
        },
      );
      setDownloadedVersionsMap((prev) => ({ ...prev, [browserStr]: versions }));
      return versions;
    } catch (error) {
      console.error("Failed to load downloaded versions:", error);
      throw error;
    }
  }, []);

  const downloadBrowser = useCallback(
    async (
      browserStr: string,
      version: string,
      suppressNotifications = false,
    ) => {
      const browserName = getBrowserDisplayName(browserStr);
      const downloadKey = `${browserStr}:${version}`;
      const existing = inFlightDownloads.get(downloadKey);
      if (existing) {
        return existing;
      }

      setDownloadingBrowsers((prev) => new Set(prev).add(browserStr));

      const downloadTask = (async () => {
        try {
          await invoke("runtime_install", {
            runtime: browserStr,
            version,
            correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
          });
          await loadDownloadedVersions(browserStr);
          setDownloadingBrowsers((prev) => {
            const next = new Set(prev);
            next.delete(browserStr);
            return next;
          });
          setDownloadProgress(null);
        } finally {
          inFlightDownloads.delete(downloadKey);
        }
      })();

      inFlightDownloads.set(downloadKey, downloadTask);

      try {
        await downloadTask;
      } catch (error) {
        console.error("Failed to download browser:", error);

        if (!suppressNotifications) {
          // Dismiss any existing download toast and show error
          dismissToast(`download-${browserStr}-${version}`);

          let errorMessage = i18n.t("common.errors.unknown");
          if (error instanceof Error) {
            errorMessage = error.message;
          } else if (typeof error === "string") {
            errorMessage = error;
          } else if (error && typeof error === "object" && "message" in error) {
            errorMessage = String(error.message);
          }

          // Ensure the long-running download toast is dismissed, and show a finite error toast
          dismissToast(`download-${browserStr}-${version}`);
          // Same id as the progress listener's error toast, so the concrete
          // backend message replaces the generic one instead of stacking on it.
          showErrorToast(
            i18n.t("browserDownload.toast.downloadFailed", {
              browser: browserName,
              version,
            }),
            {
              id: `download-error-${browserStr}-${version}`,
              description: errorMessage,
              duration: 8000,
            },
          );
        }
        throw error;
      } finally {
        setDownloadingBrowsers((prev) => {
          const next = new Set(prev);
          next.delete(browserStr);
          return next;
        });
      }
    },
    [loadDownloadedVersions],
  );

  const isVersionDownloaded = useCallback(
    (browserStr: string, version: string) => {
      return downloadedVersionsMap[browserStr]?.includes(version) ?? false;
    },
    [downloadedVersionsMap],
  );

  // Check if a browser type is currently downloading
  const isBrowserDownloading = useCallback(
    (browserStr: string) => {
      return downloadingBrowsers.has(browserStr);
    },
    [downloadingBrowsers],
  );

  // Legacy isDownloading for backwards compatibility
  const isDownloading = downloadingBrowsers.size > 0;

  // Listen for browser download progress events.
  useEffect(() => {
    let unlistenBrowser: (() => void) | null = null;

    const setupListeners = async () => {
      try {
        // Browser binaries download progress
        unlistenBrowser = await listen<DownloadProgress>(
          "download-progress",
          async (event: TauriEvent<DownloadProgress>) => {
            const progress = event.payload;
            setDownloadProgress(progress);

            const downloadKey = `${progress.browser}-${progress.version}`;
            const interruptedStage =
              lastDownloadStageRef.current.get(downloadKey);

            if (
              progress.stage === "downloading" ||
              progress.stage === "extracting" ||
              progress.stage === "verifying"
            ) {
              lastDownloadStageRef.current.set(downloadKey, progress.stage);
              setDownloadingBrowsers((prev) => {
                if (prev.has(progress.browser)) return prev;
                return new Set(prev).add(progress.browser);
              });
            } else {
              lastDownloadStageRef.current.delete(downloadKey);
            }

            const browserName = getBrowserDisplayName(progress.browser);

            if (progress.stage === "downloading") {
              const speedMBps = (
                progress.speed_bytes_per_sec /
                (1024 * 1024)
              ).toFixed(1);
              const etaText = progress.eta_seconds
                ? formatTime(progress.eta_seconds)
                : i18n.t("browserDownload.toast.calculating");

              // During first-run onboarding the welcome dialog shows browser
              // setup progress itself, so suppress the global download toast.
              if (!isOnboardingActive()) {
                showDownloadToast(
                  browserName,
                  progress.version,
                  "downloading",
                  {
                    percentage: progress.percentage,
                    speed: speedMBps,
                    eta: etaText,
                  },
                );
              }
            } else if (progress.stage === "extracting") {
              if (!isOnboardingActive()) {
                showDownloadToast(
                  browserName,
                  progress.version,
                  "extracting",
                  progress.percentage > 0
                    ? { percentage: progress.percentage }
                    : undefined,
                );
              }
            } else if (progress.stage === "verifying") {
              if (!isOnboardingActive()) {
                showDownloadToast(browserName, progress.version, "verifying");
              }
            } else if (progress.stage === "cancelled") {
              setDownloadingBrowsers((prev) => {
                const next = new Set(prev);
                next.delete(progress.browser);
                return next;
              });
              dismissToast(
                `download-${browserName.toLowerCase()}-${progress.version}`,
              );
              setDownloadProgress(null);
            } else if (progress.stage === "error") {
              setDownloadingBrowsers((prev) => {
                const next = new Set(prev);
                next.delete(progress.browser);
                return next;
              });
              dismissToast(
                `download-${browserName.toLowerCase()}-${progress.version}`,
              );
              setDownloadProgress(null);
              // During first-run onboarding the welcome dialog surfaces a
              // concrete setup error itself, so suppress the global toast.
              if (!isOnboardingActive()) {
                const wasExtracting = interruptedStage === "extracting";
                showErrorToast(
                  i18n.t(
                    wasExtracting
                      ? "browserDownload.toast.extractionFailed"
                      : "browserDownload.toast.downloadFailed",
                    {
                      browser: browserName,
                      version: progress.version,
                    },
                  ),
                  {
                    id: `download-error-${progress.browser}-${progress.version}`,
                    description: progress.error
                      ? translateBackendError(i18n.t, progress.error)
                      : i18n.t(
                          wasExtracting
                            ? "browserDownload.toast.extractionFailedDescription"
                            : "browserDownload.toast.downloadFailedDescription",
                        ),
                  },
                );
              }
            } else if (progress.stage === "completed") {
              setDownloadingBrowsers((prev) => {
                const next = new Set(prev);
                next.delete(progress.browser);
                return next;
              });
              try {
                await loadDownloadedVersions(progress.browser);
              } catch {
                /* empty */
              }
              if (!isOnboardingActive()) {
                showDownloadToast(browserName, progress.version, "completed");
              }
              setDownloadProgress(null);
            }
          },
        );
      } catch (error) {
        console.error("Failed to setup download progress listener:", error);
      }
    };

    void setupListeners();

    return () => {
      if (unlistenBrowser) {
        try {
          unlistenBrowser();
        } catch (error) {
          console.error("Failed to cleanup browser download listener:", error);
        }
      }
    };
  }, [formatTime, loadDownloadedVersions]);

  return {
    availableVersions,
    downloadedVersionsMap,
    isDownloading,
    isBrowserDownloading,
    downloadingBrowsers,
    downloadProgress,
    loadVersions,
    loadVersionsWithNewCount,
    loadDownloadedVersions,
    downloadBrowser,
    isVersionDownloaded,
    formatBytes,
    formatTime,
  };
}

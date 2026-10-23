"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

interface DownloadProgress {
  browser: string;
  version: string;
  downloaded_bytes: number;
  total_bytes: number | null;
  percentage: number;
  speed_bytes_per_sec: number;
  eta_seconds?: number | null;
  stage: string;
  error?: string;
}

export type SetupPhase = "downloading" | "extracting" | "ready" | "error";

export type SetupErrorStage =
  | "catalog"
  | "downloading"
  | "extracting"
  | "verifying"
  | "other";

export interface SetupError {
  stage: SetupErrorStage;
  /** The backend's reason, coded or plain, when it sent one. */
  detail?: string;
}

// The backend reports real extraction percentages for most archive formats
// (zip, tar.*, dmg). For formats that can't measure progress (e.g. MSI) the
// "extracting" events carry percentage 0, so we fall back to estimating from
// elapsed time vs. a learned average, seeded at 2 minutes and refined with
// the real durations we record.
const DEFAULT_EXTRACT_MS = 2 * 60 * 1000;
const MAX_SAMPLES = 5; // the 2-min seed + up to 4 most recent real durations

const storageKey = (browser: string) => `donut.extractDurations.${browser}`;

function readDurations(browser: string): number[] {
  try {
    const raw = localStorage.getItem(storageKey(browser));
    const arr = raw ? (JSON.parse(raw) as unknown) : null;
    if (
      Array.isArray(arr) &&
      arr.length > 0 &&
      arr.every((n) => typeof n === "number" && n > 0)
    ) {
      return arr as number[];
    }
  } catch {
    // fall through to the seed
  }
  return [DEFAULT_EXTRACT_MS];
}

function recordDuration(browser: string, ms: number) {
  if (!(ms > 0)) return;
  const current = readDurations(browser);
  // Keep the 2-min seed as the first value, then the most recent real samples.
  const samples =
    current[0] === DEFAULT_EXTRACT_MS ? current.slice(1) : current;
  const next = [
    DEFAULT_EXTRACT_MS,
    ...[...samples, ms].slice(-(MAX_SAMPLES - 1)),
  ];
  try {
    localStorage.setItem(storageKey(browser), JSON.stringify(next));
  } catch {
    // ignore persistence failures
  }
}

function average(values: number[]): number {
  return values.reduce((a, b) => a + b, 0) / values.length;
}

// Map a backend stage to the error stage we report when something fails.
function toErrorStage(stage: string): SetupErrorStage {
  switch (stage) {
    case "downloading":
      return "downloading";
    case "extracting":
      return "extracting";
    case "verifying":
      return "verifying";
    default:
      return "other";
  }
}

/**
 * Tracks first-launch setup of a browser: real download progress plus
 * extraction progress — real backend percentages when the archive format
 * supports them, otherwise a time-based estimate (percentages only).
 * `active` should be true while the owning dialog is open.
 */
export function useBrowserSetup(browser: string, active: boolean) {
  const [phase, setPhase] = useState<SetupPhase>("downloading");
  // Download metrics straight from the latest "downloading" event.
  const [downloadPercent, setDownloadPercent] = useState(0);
  const [downloadedBytes, setDownloadedBytes] = useState(0);
  const [totalBytes, setTotalBytes] = useState<number | null>(null);
  const [speedBytesPerSec, setSpeedBytesPerSec] = useState(0);
  const [etaSeconds, setEtaSeconds] = useState<number | null>(null);
  // Estimated extraction progress (percentages only, capped at 99 until done).
  const [extractionPercent, setExtractionPercent] = useState(0);
  const [extractionOvertime, setExtractionOvertime] = useState(false);
  const [error, setError] = useState<SetupError | null>(null);

  const extractStartRef = useRef<number | null>(null);
  const estimateRef = useRef(DEFAULT_EXTRACT_MS);
  // True once an "extracting" event carried a real percentage — from then on
  // the backend drives the bar and the time-based estimate stays out of it.
  const sawRealExtractionRef = useRef(false);
  // Fallback bookkeeping so a listener that mounts mid-flight, or that only
  // ever receives percentage-0 "extracting" events (formats that can't
  // measure progress), can still show extraction progress.
  const sawDownloadingRef = useRef(false);
  const lastProgressAtRef = useRef<number | null>(null);
  const lastDownloadPercentRef = useRef(0);
  // The last non-terminal stage we observed, used to label an error.
  const lastStageRef = useRef<string>("downloading");
  // Set once a terminal state (ready/error) is reached. Stops the tick so the
  // mid-flight extraction fallback can't re-arm and fight the readiness poll
  // (which would oscillate "ready" ↔ "Almost finished" forever).
  const doneRef = useRef(false);
  const installStartedRef = useRef(false);
  const installInFlightRef = useRef(false);
  const startInstallRef = useRef<(() => Promise<void>) | null>(null);

  useEffect(() => {
    if (!active) {
      // Fully reset when the owning dialog closes.
      setPhase("downloading");
      setDownloadPercent(0);
      setDownloadedBytes(0);
      setTotalBytes(null);
      setSpeedBytesPerSec(0);
      setEtaSeconds(null);
      setExtractionPercent(0);
      setExtractionOvertime(false);
      setError(null);
      extractStartRef.current = null;
      sawRealExtractionRef.current = false;
      sawDownloadingRef.current = false;
      lastProgressAtRef.current = null;
      lastDownloadPercentRef.current = 0;
      lastStageRef.current = "downloading";
      doneRef.current = false;
      installStartedRef.current = false;
      installInFlightRef.current = false;
      startInstallRef.current = null;
      return;
    }
    let alive = true;
    estimateRef.current = average(readDurations(browser));
    extractStartRef.current = null;
    sawRealExtractionRef.current = false;
    sawDownloadingRef.current = false;
    lastProgressAtRef.current = null;
    lastDownloadPercentRef.current = 0;
    lastStageRef.current = "downloading";
    doneRef.current = false;
    installStartedRef.current = false;
    installInFlightRef.current = false;

    const finishExtraction = () => {
      if (extractStartRef.current != null) {
        recordDuration(browser, Date.now() - extractStartRef.current);
        extractStartRef.current = null;
      }
    };

    const startInstall = async () => {
      if (!alive || installStartedRef.current || installInFlightRef.current) {
        return;
      }
      installStartedRef.current = true;
      installInFlightRef.current = true;
      let failureStage: SetupErrorStage = "catalog";
      try {
        const catalog = await invoke<{
          releases: Array<{ runtime: string; version: string }>;
        }>("runtime_catalog_current_device_get", {
          correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
        });
        const version = catalog.releases
          .filter((release) => release.runtime === browser)
          .map((release) => release.version)
          .sort((left, right) =>
            right.localeCompare(left, undefined, { numeric: true }),
          )[0];
        if (!version) {
          if (alive) {
            doneRef.current = true;
            setPhase("error");
            setError({ stage: "catalog" });
          }
          return;
        }
        failureStage = "downloading";
        await invoke("runtime_install", {
          runtime: browser,
          version,
          correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
        });
        if (alive) {
          doneRef.current = true;
          finishExtraction();
          setPhase("ready");
          setExtractionPercent(100);
          setExtractionOvertime(false);
          setError(null);
        }
      } catch (error) {
        console.error("Failed to install the first browser runtime:", error);
        if (alive) {
          doneRef.current = true;
          finishExtraction();
          setPhase("error");
          setError({
            stage:
              failureStage === "catalog"
                ? "catalog"
                : toErrorStage(lastStageRef.current),
          });
        }
      } finally {
        installInFlightRef.current = false;
      }
    };
    startInstallRef.current = startInstall;

    const unlistenPromise = listen<DownloadProgress>(
      "download-progress",
      (event) => {
        if (!alive) return;
        const p = event.payload;
        if (p.browser !== browser) return;
        switch (p.stage) {
          case "downloading":
            lastStageRef.current = "downloading";
            sawDownloadingRef.current = true;
            lastProgressAtRef.current = Date.now();
            lastDownloadPercentRef.current = p.percentage;
            setPhase("downloading");
            setDownloadPercent(Math.round(p.percentage));
            setDownloadedBytes(p.downloaded_bytes);
            setTotalBytes(p.total_bytes ?? null);
            setSpeedBytesPerSec(p.speed_bytes_per_sec);
            setEtaSeconds(p.eta_seconds ?? null);
            break;
          case "extracting":
            lastStageRef.current = "extracting";
            if (extractStartRef.current == null) {
              extractStartRef.current = Date.now();
            }
            lastProgressAtRef.current = Date.now();
            setPhase("extracting");
            if (p.percentage > 0) {
              sawRealExtractionRef.current = true;
              setExtractionPercent(Math.min(99, Math.round(p.percentage)));
              setExtractionOvertime(false);
            }
            break;
          case "verifying":
            lastStageRef.current = "verifying";
            finishExtraction();
            // Verification is the tail of extraction; keep the bar near full
            // but don't claim "ready" until "completed" arrives.
            setPhase("extracting");
            setExtractionPercent(99);
            break;
          case "completed":
            doneRef.current = true;
            finishExtraction();
            setPhase("ready");
            setExtractionPercent(100);
            setExtractionOvertime(false);
            setError(null);
            break;
          case "error":
            doneRef.current = true;
            finishExtraction();
            setPhase("error");
            setError({
              stage: toErrorStage(lastStageRef.current),
              detail: p.error,
            });
            break;
          case "cancelled":
            // Treat a cancellation like an error so the dialog can offer retry.
            doneRef.current = true;
            finishExtraction();
            setPhase("error");
            setError({ stage: "other" });
            break;
          default:
            break;
        }
      },
    );

    // Detect runtimes installed before this hook mounted or externally while it
    // is open. Once this hook starts an install, its command result and progress
    // events are authoritative; polling pauses so it cannot start a duplicate.
    let checkingReady = false;
    const checkReady = async () => {
      if (
        !alive ||
        checkingReady ||
        doneRef.current ||
        installStartedRef.current ||
        installInFlightRef.current
      ) {
        return;
      }
      checkingReady = true;
      try {
        const versions = await invoke<string[]>(
          "runtime_installed_versions_get",
          {
            runtime: browser,
            correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
          },
        );
        if (alive && versions.length > 0) {
          doneRef.current = true;
          finishExtraction();
          setPhase("ready");
          setExtractionPercent(100);
          setExtractionOvertime(false);
          setError(null);
        } else if (alive && !installStartedRef.current) {
          void startInstall();
        }
      } catch (err) {
        console.error("Failed to check browser download status:", err);
        if (alive) {
          doneRef.current = true;
          setPhase("error");
          setError({ stage: "other" });
        }
      } finally {
        checkingReady = false;
      }
    };
    void checkReady();
    const readyPoll = setInterval(() => {
      void checkReady();
    }, 1000);

    // Drive the estimated extraction percentage while extracting.
    const tick = setInterval(() => {
      if (!alive || doneRef.current) return;
      // If the download visibly finished but we never saw any "extracting"
      // event, start estimating extraction anyway — anchored to the last
      // download event, which is roughly when extraction began.
      if (
        extractStartRef.current == null &&
        sawDownloadingRef.current &&
        lastDownloadPercentRef.current >= 99 &&
        lastProgressAtRef.current != null &&
        Date.now() - lastProgressAtRef.current > 1200
      ) {
        extractStartRef.current = lastProgressAtRef.current;
        lastStageRef.current = "extracting";
        setPhase("extracting");
      }
      if (extractStartRef.current == null) return;
      // Real backend percentages drive the bar; the estimate would only
      // fight them (and flag bogus "overtime" on a healthy extraction).
      if (sawRealExtractionRef.current) return;
      const elapsed = Date.now() - extractStartRef.current;
      const est = estimateRef.current || DEFAULT_EXTRACT_MS;
      if (elapsed >= est) {
        // We've blown past the estimate — hold at 99 and flag overtime so the
        // dialog can show "Almost finished" instead of a stalled number.
        setExtractionPercent(99);
        setExtractionOvertime(true);
      } else {
        setExtractionPercent(Math.min(99, Math.round((elapsed / est) * 100)));
        setExtractionOvertime(false);
      }
    }, 250);

    return () => {
      alive = false;
      startInstallRef.current = null;
      clearInterval(tick);
      clearInterval(readyPoll);
      void unlistenPromise.then((u) => {
        u();
      });
    };
  }, [browser, active]);

  const retry = useCallback(() => {
    // Reuse the active progress listener and start a fresh catalog/install
    // attempt without restarting the effect.
    setPhase("downloading");
    setDownloadPercent(0);
    setDownloadedBytes(0);
    setTotalBytes(null);
    setSpeedBytesPerSec(0);
    setEtaSeconds(null);
    setExtractionPercent(0);
    setExtractionOvertime(false);
    setError(null);
    extractStartRef.current = null;
    sawRealExtractionRef.current = false;
    sawDownloadingRef.current = false;
    lastProgressAtRef.current = null;
    lastDownloadPercentRef.current = 0;
    lastStageRef.current = "downloading";
    doneRef.current = false;
    installStartedRef.current = false;
    void startInstallRef.current?.();
  }, []);

  return {
    phase,
    downloadPercent,
    downloadedBytes,
    totalBytes,
    speedBytesPerSec,
    etaSeconds,
    extractionPercent,
    extractionOvertime,
    ready: phase === "ready",
    error,
    retry,
  };
}

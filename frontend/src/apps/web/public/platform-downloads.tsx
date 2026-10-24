"use client";

import { useEffect, useId, useState } from "react";
import styles from "./landing.module.css";
import landingStyles from "./landing-v2.module.css";

type ReleaseManifest = {
  version: string;
  platforms: {
    windows: { url: string } | null;
    macos: { url: string } | null;
  };
};

export function PlatformDownloads({
  standalone = false,
  className,
}: {
  standalone?: boolean;
  className?: string;
}) {
  const titleId = useId();
  const [release, setRelease] = useState<ReleaseManifest | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "unavailable">(
    "loading",
  );

  useEffect(() => {
    const controller = new AbortController();
    let disposed = false;
    const refresh = async () => {
      try {
        const response = await fetch("/api/downloads/latest", {
          signal: controller.signal,
          cache: "no-store",
        });
        if (!response.ok) throw new Error("release unavailable");
        const manifest = (await response.json()) as ReleaseManifest;
        if (!disposed) {
          setRelease(manifest);
          setState("ready");
        }
      } catch {
        if (!disposed && !controller.signal.aborted) setState("unavailable");
      }
    };
    const refreshWhenVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), 5 * 60 * 1000);
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      disposed = true;
      controller.abort();
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, []);

  const platformHint =
    typeof navigator !== "undefined"
      ? `${navigator.userAgent} ${navigator.platform}`
      : "";
  const primaryPlatform = /mac/i.test(platformHint) ? "macos" : "windows";
  const primary = release?.platforms[primaryPlatform];
  const heading = standalone ? "Tải BugLogin Desktop" : "Tải BugLogin";
  const view = standalone ? styles : landingStyles;

  return (
    <section
      className={`${standalone ? styles.downloadPage : landingStyles.downloadBlock} ${className ?? ""}`}
      aria-labelledby={titleId}
    >
      <p className={view.eyebrow}>BẢN PHÁT HÀNH ỔN ĐỊNH MỚI NHẤT</p>
      <h2 id={titleId}>{heading}</h2>
      {state === "ready" && release ? (
        <>
          <p className={view.releaseVersion}>BugLogin v{release.version}</p>
          <div className={view.downloadActions}>
            {primary ? (
              <a
                className={view.buttonPrimary}
                href={`/api/downloads/latest?platform=${primaryPlatform}`}
              >
                <span aria-hidden="true">
                  {primaryPlatform === "macos" ? "⌘" : "⊞"}
                </span>
                {primaryPlatform === "macos"
                  ? "Tải cho macOS"
                  : "Tải cho Windows"}
                <span aria-hidden="true">↓</span>
              </a>
            ) : null}
            {release.platforms.windows ? (
              <a
                className={view.buttonSecondary}
                href="/api/downloads/latest?platform=windows"
              >
                {primaryPlatform === "windows"
                  ? "Windows · đã chọn"
                  : "Windows"}
              </a>
            ) : null}
            {release.platforms.macos ? (
              <a
                className={view.buttonSecondary}
                href="/api/downloads/latest?platform=macos"
              >
                {primaryPlatform === "macos"
                  ? "macOS · đã chọn"
                  : "macOS · Apple Silicon"}
              </a>
            ) : null}
          </div>
          {!primary ? (
            <p className={view.downloadStatus} role="status">
              Chưa có gói tải ổn định cho hệ điều hành này.
            </p>
          ) : null}
        </>
      ) : (
        <p
          className={view.downloadStatus}
          role={state === "unavailable" ? "status" : undefined}
        >
          {state === "loading"
            ? "Đang kiểm tra bản tải mới nhất…"
            : "Link tải ổn định hiện chưa sẵn sàng. Vui lòng quay lại sau."}
        </p>
      )}
    </section>
  );
}

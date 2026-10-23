"use client";

import { useEffect, useState } from "react";
import styles from "./landing.module.css";

type ReleaseManifest = {
  version: string;
  platforms: {
    windows: { url: string } | null;
    macos: { url: string } | null;
  };
};

export function PlatformDownloads({
  standalone = false,
}: {
  standalone?: boolean;
}) {
  const [release, setRelease] = useState<ReleaseManifest | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "unavailable">(
    "loading",
  );

  useEffect(() => {
    const controller = new AbortController();
    fetch("/api/downloads/latest", {
      signal: controller.signal,
      cache: "no-store",
    })
      .then(async (response) => {
        if (!response.ok) throw new Error("release unavailable");
        return (await response.json()) as ReleaseManifest;
      })
      .then((manifest) => {
        setRelease(manifest);
        setState("ready");
      })
      .catch(() => {
        if (!controller.signal.aborted) setState("unavailable");
      });
    return () => controller.abort();
  }, []);

  const primaryPlatform =
    typeof navigator !== "undefined" && /mac/i.test(navigator.userAgent)
      ? "macos"
      : "windows";
  const primary = release?.platforms[primaryPlatform];
  const heading = standalone ? "Tải BugLogin Desktop" : "Tải BugLogin";

  return (
    <section
      className={standalone ? styles.downloadPage : styles.downloadBlock}
      aria-labelledby="downloads-title"
    >
      <p className={styles.eyebrow}>BẢN PHÁT HÀNH MỚI NHẤT</p>
      <h2 id="downloads-title">{heading}</h2>
      {state === "ready" && release ? (
        <>
          <p className={styles.releaseVersion}>BugLogin v{release.version}</p>
          <div className={styles.downloadActions}>
            {primary ? (
              <a
                className={styles.buttonPrimary}
                target="_blank"
                rel="noreferrer"
                href={`/api/downloads/latest?platform=${primaryPlatform}`}
              >
                {primaryPlatform === "macos"
                  ? "Tải cho macOS"
                  : "Tải cho Windows"}
                <span aria-hidden="true"> ↓</span>
              </a>
            ) : null}
            {release.platforms.windows ? (
              <a
                className={styles.buttonSecondary}
                target="_blank"
                rel="noreferrer"
                href="/api/downloads/latest?platform=windows"
              >
                Windows
              </a>
            ) : null}
            {release.platforms.macos ? (
              <a
                className={styles.buttonSecondary}
                target="_blank"
                rel="noreferrer"
                href="/api/downloads/latest?platform=macos"
              >
                macOS (Apple Silicon)
              </a>
            ) : null}
          </div>
          {!primary ? (
            <p className={styles.downloadStatus}>
              Bản tải cho hệ điều hành này chưa được phát hành.
            </p>
          ) : null}
        </>
      ) : (
        <p
          className={styles.downloadStatus}
          role={state === "unavailable" ? "status" : undefined}
        >
          {state === "loading"
            ? "Đang kiểm tra bản phát hành…"
            : "Chưa có bản phát hành ổn định để tải. Vui lòng quay lại sau."}
        </p>
      )}
    </section>
  );
}

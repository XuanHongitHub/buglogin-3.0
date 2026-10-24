import { useCallback, useEffect, useState } from "react";
import type { Profile, ProfileConnection, WebApiClient } from "./api";
import styles from "./profile-detail.module.css";
import { useLoadState } from "./shells";

const neverEmpty = () => false;
const viewerRedeems = new Map<string, Promise<void>>();

export function ProfileDetailPage({
  api,
  profileId,
}: {
  api: WebApiClient;
  profileId: string;
}) {
  const [handoff, setHandoff] = useState<
    | { status: "checking" }
    | { status: "ready" }
    | { status: "error"; message: string }
  >({ status: "checking" });

  useEffect(() => {
    const fragment = new URLSearchParams(window.location.hash.slice(1));
    const ticket = fragment.get("viewer");
    if (!ticket) {
      setHandoff({ status: "ready" });
      return;
    }
    window.history.replaceState(
      window.history.state,
      "",
      `${window.location.pathname}${window.location.search}`,
    );
    let active = true;
    const key = `${profileId}:${ticket}`;
    let redemption = viewerRedeems.get(key);
    if (!redemption) {
      redemption = api.writes
        .redeemProfileViewerTicket(
          profileId,
          ticket,
          new AbortController().signal,
        )
        .finally(() => viewerRedeems.delete(key));
      viewerRedeems.set(key, redemption);
    }
    redemption
      .then(() => {
        if (active) setHandoff({ status: "ready" });
      })
      .catch((error: unknown) => {
        if (active)
          setHandoff({
            status: "error",
            message:
              error instanceof Error
                ? error.message
                : "Liên kết mở profile không hợp lệ hoặc đã hết hạn.",
          });
      });
    return () => {
      active = false;
    };
  }, [api, profileId]);

  if (handoff.status === "checking")
    return (
      <main className={styles.page} aria-live="polite">
        <p>Đang xác thực quyền xem profile…</p>
      </main>
    );
  if (handoff.status === "error")
    return (
      <main className={styles.page}>
        <section className={styles.notice} role="alert">
          <h1>Không mở được profile</h1>
          <p>{handoff.message}</p>
          <p>Hãy đóng cửa sổ này và mở profile lại từ BugLogin.</p>
        </section>
      </main>
    );
  return <ProfileDetailContent api={api} profileId={profileId} />;
}

function ProfileDetailContent({
  api,
  profileId,
}: {
  api: WebApiClient;
  profileId: string;
}) {
  const loadProfile = useCallback(
    (signal: AbortSignal) => api.reads.profile(profileId, signal),
    [api, profileId],
  );
  const { state, retry } = useLoadState(
    loadProfile,
    neverEmpty,
    "Profile không tồn tại hoặc bạn không có quyền xem profile này.",
  );
  const [connection, setConnection] = useState<
    | { status: "loading" }
    | { status: "ready"; data: ProfileConnection }
    | { status: "unavailable" }
  >({ status: "loading" });

  useEffect(() => {
    const controller = new AbortController();
    setConnection({ status: "loading" });
    api.reads
      .profileConnection(controller.signal)
      .then((data) => setConnection({ status: "ready", data }))
      .catch(() => {
        if (!controller.signal.aborted)
          setConnection({ status: "unavailable" });
      });
    return () => controller.abort();
  }, [api]);

  if (state.status === "loading")
    return (
      <main className={styles.page} aria-live="polite">
        <p>Đang tải dữ liệu profile…</p>
      </main>
    );
  if (state.status !== "ready")
    return (
      <main className={styles.page}>
        <section className={styles.notice} role="alert">
          <h1>
            {state.status === "error"
              ? "Không tải được profile"
              : "Không tìm thấy profile"}
          </h1>
          <p>{state.status === "error" ? state.message : state.message}</p>
          <button type="button" onClick={retry}>
            Thử lại
          </button>
        </section>
      </main>
    );

  return (
    <ProfileDetails
      profile={state.data.value}
      profileId={profileId}
      connection={connection}
    />
  );
}

function ProfileDetails({
  profile,
  profileId,
  connection,
}: {
  profile: Profile;
  profileId: string;
  connection:
    | { status: "loading" }
    | { status: "ready"; data: ProfileConnection }
    | { status: "unavailable" };
}) {
  const [copied, setCopied] = useState(false);
  const ip = connection.status === "ready" ? connection.data.ip : null;
  const country =
    connection.status === "ready" ? connection.data.countryCode : null;
  const os = profileOperatingSystem(profile);
  const fingerprint = fingerprintFacts(profile);
  const copyLink = async () => {
    try {
      await navigator.clipboard.writeText(window.location.href);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      setCopied(false);
    }
  };

  return (
    <main className={styles.page}>
      <div className={styles.topline}>
        <a href="/" className={styles.brand}>
          <img src="/buglogin-logo.png" alt="" />
          BugLogin
        </a>
        <span>PROFILE DETAIL</span>
      </div>
      <header className={styles.header}>
        <div className={styles.heading}>
          <p className={styles.kicker}>BROWSER PROFILE</p>
          <h1>{profile.name}</h1>
          <div className={styles.meta}>
            <span className={styles.state}>
              <b />
              {profileStateLabel(profile.state)}
            </span>
            <span>
              Browser · {profile.runtimeVersion ?? "Version chưa xác định"}
            </span>
            {os ? <span>{os}</span> : null}
          </div>
        </div>
        <button type="button" className={styles.copyButton} onClick={copyLink}>
          {copied ? "Đã sao chép link" : "Sao chép link profile"}
        </button>
      </header>

      <section className={styles.grid} aria-label="Thông tin profile">
        <article className={styles.panel}>
          <div className={styles.panelHeading}>
            <span className={styles.marker}>01</span>
            <div>
              <h2>Kết nối hiện tại</h2>
              <p>IP quan sát từ tab profile này</p>
            </div>
          </div>
          <div className={styles.ipRow}>
            <strong>{ip ?? "Chưa quan sát được IP"}</strong>
            {country ? (
              <span
                className={`${styles.country} fi fi-${country.toLowerCase()}`}
                role="img"
                aria-label={`Cờ ${country}`}
                title={country}
              />
            ) : null}
          </div>
          <p className={styles.countryName}>
            {country
              ? countryName(country)
              : "Quốc gia sẽ hiện khi request đi qua Cloudflare edge."}
          </p>
          <div className={styles.rule} />
          <dl className={styles.facts}>
            <Fact label="Quốc gia / vùng" value={country ?? "Chưa xác minh"} />
            <Fact
              label="Proxy gắn với profile"
              value={profile.proxyId ? "Đã cấu hình" : "Không có"}
            />
            <Fact
              label="VPN"
              value={profile.vpnId ? "Đã cấu hình" : "Không có"}
            />
            <Fact
              label="Chế độ đồng bộ"
              value={profile.syncMode ?? "Chưa cấu hình"}
            />
          </dl>
          <p className={styles.caption}>
            {connection.status === "loading"
              ? "Đang đọc IP từ Cloudflare edge…"
              : connection.status === "unavailable"
                ? "Không đọc được thông tin mạng. Dữ liệu profile vẫn được giữ nguyên."
                : "IP và quốc gia do Cloudflare quan sát tại thời điểm trang được mở; không phải dữ liệu định vị lưu trong profile."}
          </p>
        </article>

        <article className={styles.panel}>
          <div className={styles.panelHeading}>
            <span className={styles.marker}>02</span>
            <div>
              <h2>Môi trường browser</h2>
              <p>Cấu hình đã lưu trong profile</p>
            </div>
          </div>
          <dl className={styles.facts}>
            <Fact label="Browser" value="BugLogin Browser" />
            <Fact label="Runtime" value={runtimeLabel(profile.runtime)} />
            <Fact
              label="Phiên bản"
              value={profile.runtimeVersion ?? "Chưa xác định"}
            />
            <Fact
              label="Hệ điều hành mục tiêu"
              value={os ?? "Mặc định runtime"}
            />
            <Fact
              label="Múi giờ fingerprint"
              value={fingerprint.timezone ?? "Chưa cấu hình"}
            />
            <Fact
              label="Ngôn ngữ fingerprint"
              value={fingerprint.language ?? "Chưa cấu hình"}
            />
            <Fact
              label="Độ phân giải"
              value={fingerprint.screen ?? "Chưa cấu hình"}
            />
            <Fact
              label="CPU / RAM"
              value={
                [fingerprint.cores, fingerprint.memory]
                  .filter(Boolean)
                  .join(" / ") || "Chưa cấu hình"
              }
            />
          </dl>
          <div className={styles.signalList}>
            <Signal
              name="Canvas"
              value={fingerprint.canvas ?? "Không công bố"}
            />
            <Signal name="WebGL" value={fingerprint.webgl ?? "Không công bố"} />
            <Signal name="Audio" value={fingerprint.audio ?? "Không công bố"} />
            <Signal
              name="WebRTC"
              value={fingerprint.webrtc ?? "Không công bố"}
            />
          </div>
        </article>
      </section>

      <footer className={styles.footer}>
        <span>
          PROFILE ID <code>{profileId}</code>
        </span>
        <span>Cập nhật {new Date(profile.updatedAt).toLocaleString()}</span>
      </footer>
    </main>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.fact}>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function Signal({ name, value }: { name: string; value: string }) {
  return (
    <div className={styles.signal}>
      <span>{name}</span>
      <strong>{value}</strong>
    </div>
  );
}

function profileOperatingSystem(profile: Profile): string | null {
  const config = asRecord(profile.runtimeConfig);
  const wayfern = asRecord(config?.wayfern);
  const target = asRecord(wayfern?.fingerprintTarget);
  const os = target?.operatingSystem;
  if (typeof os !== "string") return null;
  return (
    (
      {
        windows: "Windows",
        macos: "macOS",
        linux: "Linux",
        android: "Android",
        ios: "iOS",
      } as Record<string, string>
    )[os] ?? null
  );
}

function fingerprintFacts(profile: Profile) {
  const root = asRecord(profile.fingerprint);
  const wayfern = asRecord(root?.wayfern) ?? root;
  const text = (key: string) =>
    typeof wayfern?.[key] === "string" ? (wayfern[key] as string) : null;
  const number = (key: string) =>
    typeof wayfern?.[key] === "number" ? String(wayfern[key]) : null;
  const screen =
    number("screenWidth") && number("screenHeight")
      ? `${number("screenWidth")} × ${number("screenHeight")}`
      : null;
  return {
    timezone: text("timezone"),
    language: text("language"),
    screen,
    cores: number("hardwareConcurrency")
      ? `${number("hardwareConcurrency")} cores`
      : null,
    memory: number("deviceMemory") ? `${number("deviceMemory")} GB` : null,
    canvas: text("canvas") ?? (text("canvasNoiseSeed") ? "Đã cấu hình" : null),
    webgl: text("webglRenderer") ?? text("webgl"),
    audio: text("audio") ?? (number("audioSampleRate") ? "Đã cấu hình" : null),
    webrtc: text("webrtc"),
  };
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function profileStateLabel(state: Profile["state"]): string {
  return {
    created: "Đã tạo",
    ready: "Sẵn sàng",
    launching: "Đang mở",
    running: "Đang chạy",
    stopping: "Đang đóng",
    failed: "Có lỗi",
    recovering: "Đang khôi phục",
  }[state];
}

function runtimeLabel(runtime: Profile["runtime"]): string {
  return runtime === "wayfern" ? "Browser runtime" : runtime;
}

function countryName(code: string): string {
  try {
    return new Intl.DisplayNames(["vi"], { type: "region" }).of(code) ?? code;
  } catch {
    return code;
  }
}

import { NextResponse } from "next/server";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const CATALOG_BASE =
  "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3";
const ARTIFACT_HOST = "mavelnut-login.sgp1.digitaloceanspaces.com";
const ARTIFACT_PREFIX = "/releases/desktop/v3/artifacts/";

type Feed = {
  version?: unknown;
  platforms?: Record<string, unknown>;
};

type Release = { feed: Feed; channel: "stable" | "rc" };

async function loadRelease(): Promise<Release | null> {
  for (const channel of ["stable", "rc"] as const) {
    try {
      const response = await fetch(
        `${CATALOG_BASE}/${channel}/downloads.json`,
        {
          cache: "no-store",
          headers: { accept: "application/json", "cache-control": "no-cache" },
          signal: AbortSignal.timeout(8000),
        },
      );
      if (!response.ok) continue;
      const contentLength = Number(response.headers.get("content-length") ?? 0);
      if (contentLength > 1024 * 1024) continue;
      const body = await response.text();
      if (body.length > 1024 * 1024) continue;
      const feed = JSON.parse(body) as Feed;
      if (
        typeof feed.version === "string" &&
        /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(feed.version) &&
        installerUrl(feed.platforms?.windows, feed.version, "windows") &&
        installerUrl(feed.platforms?.macos, feed.version, "macos")
      ) {
        return { feed, channel };
      }
    } catch {
      // Try the next published channel; availability is validated below.
    }
  }
  return null;
}

function installerUrl(
  value: unknown,
  version: string,
  platform: "windows" | "macos",
): string | null {
  if (!value || typeof value !== "object") return null;
  const item = value as { url?: unknown };
  if (typeof item.url !== "string") return null;
  try {
    const url = new URL(item.url);
    const pathParts = url.pathname.startsWith(ARTIFACT_PREFIX)
      ? url.pathname.slice(ARTIFACT_PREFIX.length).split("/")
      : [];
    const filename =
      pathParts.length === 3 ? decodeURIComponent(pathParts[2]) : "";
    const expectedFilename =
      platform === "windows"
        ? filename === `BugLogin_${version}_x64-setup.exe`
        : filename === `BugLogin_${version}_aarch64.dmg`;
    if (
      url.protocol !== "https:" ||
      url.hostname !== ARTIFACT_HOST ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      pathParts.length !== 3 ||
      pathParts[0] !== `v${version}` ||
      !/^\d+$/.test(pathParts[1]) ||
      !expectedFilename
    )
      return null;
    return url.toString();
  } catch {
    return null;
  }
}

export async function GET(request: Request) {
  const release = await loadRelease();
  if (!release) {
    return NextResponse.json(
      { error: "release_unavailable" },
      {
        status: 503,
        headers: { "Cache-Control": "no-store" },
      },
    );
  }
  const { feed, channel } = release;

  if (typeof feed.version !== "string") {
    return NextResponse.json(
      { error: "invalid_release_manifest" },
      { status: 502 },
    );
  }

  const platforms = {
    windows: installerUrl(feed.platforms?.windows, feed.version, "windows"),
    macos: installerUrl(feed.platforms?.macos, feed.version, "macos"),
  };
  const platform = new URL(request.url).searchParams.get("platform");
  if (platform !== null) {
    if (platform !== "windows" && platform !== "macos") {
      return NextResponse.json(
        { error: "unsupported_platform" },
        { status: 400 },
      );
    }
    const url = platforms[platform];
    if (!url)
      return NextResponse.json(
        { error: "platform_release_unavailable" },
        { status: 404 },
      );
    return NextResponse.redirect(url, 302);
  }

  return NextResponse.json(
    {
      version: feed.version,
      channel,
      platforms: {
        windows: platforms.windows ? { url: platforms.windows } : null,
        macos: platforms.macos ? { url: platforms.macos } : null,
      },
    },
    {
      headers: { "Cache-Control": "no-store" },
    },
  );
}

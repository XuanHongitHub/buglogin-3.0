import { NextResponse } from "next/server";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const FEED_BASE_URL =
  "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3";
const ARTIFACT_HOST = "mavelnut-login.sgp1.digitaloceanspaces.com";
const ARTIFACT_PREFIX = "/releases/desktop/v3/artifacts/";

type DownloadManifest = {
  version?: unknown;
  platforms?: Record<string, unknown>;
};

function artifactUrl(
  value: unknown,
  version: string,
  platform: "windows" | "macos",
): string | null {
  if (typeof value !== "string") return null;
  try {
    const url = new URL(value);
    const expectedSuffix = platform === "windows" ? "-setup.exe" : ".dmg";
    const relativePath = url.pathname.startsWith(ARTIFACT_PREFIX)
      ? url.pathname.slice(ARTIFACT_PREFIX.length).split("/")
      : [];
    const filename = decodeURIComponent(relativePath[2] ?? "");
    if (
      url.protocol !== "https:" ||
      url.hostname !== ARTIFACT_HOST ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      relativePath.length !== 3 ||
      relativePath[0] !== `v${version}` ||
      !/^\d+$/.test(relativePath[1]) ||
      !filename.startsWith(`BugLogin_${version}_`) ||
      /[/\\\r\n]/.test(filename) ||
      !filename.endsWith(expectedSuffix)
    )
      return null;
    return url.toString();
  } catch {
    return null;
  }
}

export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const channel = params.get("channel") ?? "stable";
  if (channel !== "stable" && channel !== "rc") {
    return NextResponse.json({ error: "unsupported_channel" }, { status: 400 });
  }
  let feed: DownloadManifest;
  try {
    const response = await fetch(`${FEED_BASE_URL}/${channel}/downloads.json`, {
      cache: "no-store",
      headers: { accept: "application/json", "cache-control": "no-cache" },
      signal: AbortSignal.timeout(8000),
    });
    const body = await response.text();
    if (new TextEncoder().encode(body).byteLength > 1024 * 1024) {
      return NextResponse.json(
        { error: "invalid_release_manifest" },
        { status: 502 },
      );
    }
    if (!response.ok) {
      return NextResponse.json(
        { error: "release_unavailable" },
        {
          status: response.status === 404 ? 404 : 503,
          headers: { "Cache-Control": "no-store" },
        },
      );
    }
    feed = JSON.parse(body) as DownloadManifest;
  } catch {
    return NextResponse.json(
      { error: "release_unavailable" },
      {
        status: 503,
        headers: { "Cache-Control": "no-store" },
      },
    );
  }

  if (
    typeof feed.version !== "string" ||
    !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(feed.version)
  ) {
    return NextResponse.json(
      { error: "invalid_release_manifest" },
      { status: 502 },
    );
  }

  const platforms = {
    windows: artifactUrl(
      (feed.platforms?.windows as { url?: unknown } | undefined)?.url,
      feed.version,
      "windows",
    ),
    macos: artifactUrl(
      (feed.platforms?.macos as { url?: unknown } | undefined)?.url,
      feed.version,
      "macos",
    ),
  };
  const platform = params.get("platform");
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

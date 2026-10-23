import { NextResponse } from "next/server";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const FEED_URL =
  "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/stable/latest.json";
const ARTIFACT_HOST = "mavelnut-login.sgp1.digitaloceanspaces.com";
const ARTIFACT_PREFIX = "/releases/desktop/v3/artifacts/";

type Feed = {
  version?: unknown;
  downloads?: Record<string, unknown>;
};

function artifactUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const url = new URL(value);
    if (
      url.protocol !== "https:" ||
      url.hostname !== ARTIFACT_HOST ||
      url.username ||
      url.password ||
      !url.pathname.startsWith(ARTIFACT_PREFIX)
    )
      return null;
    return url.toString();
  } catch {
    return null;
  }
}

export async function GET(request: Request) {
  let feed: Feed;
  try {
    const response = await fetch(FEED_URL, {
      cache: "no-store",
      headers: { accept: "application/json", "cache-control": "no-cache" },
      signal: AbortSignal.timeout(8000),
    });
    const contentLength = Number(response.headers.get("content-length") ?? 0);
    if (contentLength > 1024 * 1024) {
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
    feed = (await response.json()) as Feed;
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
      (feed.downloads?.windows as { url?: unknown } | undefined)?.url,
    ),
    macos: artifactUrl(
      (feed.downloads?.macos as { url?: unknown } | undefined)?.url,
    ),
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

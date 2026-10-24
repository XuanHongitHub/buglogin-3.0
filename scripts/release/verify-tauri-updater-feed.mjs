import { readFile } from "node:fs/promises";

const [manifestPath, version, repository, artifactRootArg] =
  process.argv.slice(2);
if (!manifestPath || !version || !repository) {
  throw new Error(
    "usage: verify-tauri-updater-feed.mjs <latest.json> <version> <owner/repo> [artifact-root-url]",
  );
}
if (!/^\d+\.\d+\.\d+$/.test(version))
  throw new Error("stable updater feed requires a final semantic version");
if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository))
  throw new Error("invalid GitHub repository");

const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const artifactRoot = new URL(
  artifactRootArg || `https://github.com/${repository}/releases/download/`,
);
if (
  artifactRoot.protocol !== "https:" ||
  artifactRoot.username ||
  artifactRoot.password ||
  artifactRoot.search ||
  artifactRoot.hash ||
  !artifactRoot.pathname.endsWith("/")
) {
  throw new Error(
    "artifact root URL must be credential-free HTTPS and end with a slash",
  );
}
const required = {
  "windows-x86_64": ".exe",
  "darwin-aarch64": ".app.tar.gz",
};
if (
  manifest.version !== version ||
  Object.keys(manifest.platforms ?? {})
    .sort()
    .join("|") !== Object.keys(required).sort().join("|") ||
  Object.keys(manifest.downloads ?? {})
    .sort()
    .join("|") !== "macos|windows"
) {
  throw new Error(
    "updater feed version or platform coverage does not match this stable release",
  );
}

function artifactPath(urlValue, suffix, platform) {
  const url = new URL(urlValue);
  const expectedPrefix = new URL(`v${version}/`, artifactRoot).pathname;
  const urlParts = url.pathname.startsWith(expectedPrefix)
    ? url.pathname.slice(expectedPrefix.length).split("/")
    : [];
  const validArtifactPath =
    urlParts.length === 1 ||
    (urlParts.length === 2 && /^[0-9]+$/.test(urlParts[0]));
  if (
    url.protocol !== "https:" ||
    url.origin !== artifactRoot.origin ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !validArtifactPath
  ) {
    throw new Error(
      `${platform}: download URL does not reference this immutable release tag`,
    );
  }
  const name = decodeURIComponent(urlParts.at(-1));
  if (!name.endsWith(suffix) || name.includes("/") || name.includes("\\")) {
    throw new Error(`${platform}: unexpected download artifact name`);
  }
  return { url, urlParts, name };
}

for (const [platform, requiredSuffix] of Object.entries(required)) {
  const entry = manifest.platforms[platform];
  if (
    typeof entry?.signature !== "string" ||
    !entry.signature.trim() ||
    /[\r\n]/.test(entry.signature)
  ) {
    throw new Error(`${platform}: invalid detached updater signature`);
  }
  const updater = artifactPath(entry.url, requiredSuffix, platform);
  const downloadKey = platform === "windows-x86_64" ? "windows" : "macos";
  const downloadSuffix = platform === "windows-x86_64" ? ".exe" : ".dmg";
  const download = artifactPath(
    manifest.downloads[downloadKey]?.url,
    downloadSuffix,
    `${platform} download`,
  );
  const sameImmutableRelease =
    download.urlParts.length === updater.urlParts.length &&
    (download.urlParts.length === 1 ||
      download.urlParts[0] === updater.urlParts[0]);
  if (!sameImmutableRelease || download.url.origin !== updater.url.origin) {
    throw new Error(
      `${platform}: installer and updater must come from the same immutable release run`,
    );
  }
  if (
    platform === "windows-x86_64" &&
    manifest.downloads.windows.url !== entry.url
  ) {
    throw new Error(
      "Windows installer link must match the signed NSIS updater artifact",
    );
  }
}
console.log(
  `Verified stable updater feed for ${version} (${Object.keys(required).length} platforms)`,
);

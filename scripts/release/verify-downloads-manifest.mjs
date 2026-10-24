import { readFile } from "node:fs/promises";

const [manifestPath, version, artifactRootArg] = process.argv.slice(2);
if (!manifestPath || !version || !artifactRootArg) {
  throw new Error(
    "usage: verify-downloads-manifest.mjs <downloads.json> <version> <immutable-artifact-root-url>",
  );
}
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error("downloads manifest requires a final semantic version");
}

const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const artifactRoot = new URL(artifactRootArg);
if (
  artifactRoot.protocol !== "https:" ||
  artifactRoot.username ||
  artifactRoot.password ||
  artifactRoot.search ||
  artifactRoot.hash ||
  !artifactRoot.pathname.endsWith("/")
) {
  throw new Error("artifact root must be credential-free HTTPS and end with /");
}
if (
  Object.keys(manifest).sort().join("|") !== "platforms|version" ||
  manifest.version !== version ||
  Object.keys(manifest.platforms ?? {}).sort().join("|") !== "macos|windows"
) {
  throw new Error("downloads manifest version or platform coverage is invalid");
}

for (const [platform, suffix] of Object.entries({
  windows: ".exe",
  macos: ".dmg",
})) {
  const url = new URL(manifest.platforms[platform]?.url);
  const relativePath = url.pathname.startsWith(artifactRoot.pathname)
    ? url.pathname.slice(artifactRoot.pathname.length)
    : "";
  if (
    url.protocol !== "https:" ||
    url.origin !== artifactRoot.origin ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !relativePath ||
    relativePath.includes("/") ||
    relativePath.includes("\\") ||
    !decodeURIComponent(relativePath).endsWith(suffix)
  ) {
    throw new Error(`${platform}: installer URL is outside the immutable release`);
  }
}

console.log(
  `Verified manual download manifest for ${version} (Windows and macOS)`,
);

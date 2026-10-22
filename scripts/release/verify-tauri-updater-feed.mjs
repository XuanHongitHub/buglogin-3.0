import { readFile } from "node:fs/promises";

const [manifestPath, version, repository] = process.argv.slice(2);
if (!manifestPath || !version || !repository) {
  throw new Error("usage: verify-tauri-updater-feed.mjs <latest.json> <version> <owner/repo>");
}
if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error("stable updater feed requires a final semantic version");
if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error("invalid GitHub repository");

const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const required = {
  "windows-x86_64": ".nsis.zip",
  "darwin-aarch64": ".app.tar.gz",
  "darwin-x86_64": ".app.tar.gz",
};
if (manifest.version !== version || Object.keys(manifest.platforms ?? {}).sort().join("|") !== Object.keys(required).sort().join("|")) {
  throw new Error("updater feed version or platform coverage does not match this stable release");
}

for (const [platform, suffix] of Object.entries(required)) {
  const entry = manifest.platforms[platform];
  if (typeof entry?.signature !== "string" || !entry.signature.trim() || /[\r\n]/.test(entry.signature)) {
    throw new Error(`${platform}: invalid detached updater signature`);
  }
  const url = new URL(entry.url);
  const expectedPath = `/download/v${version}/`;
  if (url.protocol !== "https:" || url.hostname !== "github.com" || url.pathname.split(expectedPath).length !== 2 || !url.pathname.includes(expectedPath)) {
    throw new Error(`${platform}: updater URL does not reference this immutable release tag`);
  }
  const name = decodeURIComponent(url.pathname.slice(url.pathname.lastIndexOf("/") + 1));
  if (!name.endsWith(suffix) || name.includes("/") || name.includes("\\")) {
    throw new Error(`${platform}: unexpected updater artifact name`);
  }
}
console.log(`Verified stable updater feed for ${version} (${Object.keys(required).length} platforms)`);

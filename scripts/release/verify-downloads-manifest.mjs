import { readFile } from "node:fs/promises";

const [manifestPath, version, artifactRootArg] = process.argv.slice(2);
if (!manifestPath || !version || !artifactRootArg) {
  throw new Error("usage: verify-downloads-manifest.mjs <downloads.json> <version> <artifact-root-url>");
}
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const artifactRoot = new URL(artifactRootArg);
if (artifactRoot.protocol !== "https:" || artifactRoot.username || artifactRoot.password || artifactRoot.search || artifactRoot.hash || !artifactRoot.pathname.endsWith("/")) {
  throw new Error("artifact root must be credential-free HTTPS and end with a slash");
}
if (manifest.version !== version || Object.keys(manifest.platforms ?? {}).sort().join("|") !== "macos|windows") {
  throw new Error("download manifest version or platform coverage is invalid");
}
for (const [platform, suffix] of [["windows", "-setup.exe"], ["macos", ".dmg"]]) {
  const item = manifest.platforms[platform];
  if (typeof item?.filename !== "string" || !item.filename.endsWith(suffix) || /[/\\\r\n]/.test(item.filename)) {
    throw new Error(`${platform}: invalid installer filename`);
  }
  const url = new URL(item.url);
  const relative = url.pathname.startsWith(artifactRoot.pathname) ? url.pathname.slice(artifactRoot.pathname.length).split("/") : [];
  if (url.protocol !== "https:" || url.origin !== artifactRoot.origin || url.username || url.password || url.search || url.hash || relative.length !== 3 || relative[0] !== `v${version}` || !/^\d+$/.test(relative[1]) || decodeURIComponent(relative[2]) !== item.filename) {
    throw new Error(`${platform}: installer URL is outside the immutable version artifact prefix`);
  }
}
console.log(`Verified Windows and macOS installer URLs for ${version}`);

import { copyFile, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

const [inputRootArg, outputRootArg, version, repository, artifactBaseUrlArg] = process.argv.slice(2);
if (!inputRootArg || !outputRootArg || !version || !repository) {
  throw new Error("usage: assemble-tauri-updater-feed.mjs <incoming-root> <output-root> <version> <owner/repo> [artifact-base-url]");
}
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error("invalid updater version");
if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error("invalid GitHub repository");

const inputRoot = resolve(inputRootArg);
const outputRoot = resolve(outputRootArg);
const artifactBaseUrl = new URL(artifactBaseUrlArg || `https://github.com/${repository}/releases/download/v${version}/`);
const versionPath = version.replaceAll(".", "\\.").replaceAll("+", "\\+");
if (artifactBaseUrl.protocol !== "https:" || artifactBaseUrl.username || artifactBaseUrl.password || artifactBaseUrl.search || artifactBaseUrl.hash || !new RegExp(`/v${versionPath}/(?:[0-9]+/)?$`).test(artifactBaseUrl.pathname)) {
  throw new Error("artifact base URL must be credential-free HTTPS and end with /v<version>/");
}
const platforms = [
  { directory: `buglogin-v${version}-windows-x64-signed`, key: "windows-x86_64", suffix: ".exe", installerSuffix: "-setup.exe", downloadKey: "windows" },
  { directory: `buglogin-v${version}-macos-arm64-signed`, key: "darwin-aarch64", suffix: ".app.tar.gz", installerSuffix: ".dmg", downloadKey: "macos" },
];
const manifestPlatforms = {};
const downloadPlatforms = {};
const assets = [];

for (const platform of platforms) {
  const directory = join(inputRoot, platform.directory);
  const files = await readdir(directory, { withFileTypes: true });
  const candidates = files
    .filter((file) => file.isFile() && file.name.endsWith(platform.suffix))
    .map((file) => file.name);
  if (candidates.length !== 1) throw new Error(`${platform.key}: expected exactly one ${platform.suffix} artifact`);
  const artifactName = candidates[0];
  const installers = files
    .filter((file) => file.isFile() && file.name.endsWith(platform.installerSuffix))
    .map((file) => file.name);
  if (installers.length !== 1) throw new Error(`${platform.key}: expected exactly one ${platform.installerSuffix} installer`);
  const installerName = installers[0];
  const signatureName = `${artifactName}.sig`;
  const signature = (await readFile(join(directory, signatureName), "utf8")).trim();
  if (!signature || signature.length > 4096 || /[\r\n]/.test(signature)) {
    throw new Error(`${platform.key}: missing or malformed detached Tauri signature`);
  }
  const artifactPath = join(directory, artifactName);
  const artifactBytes = await readFile(artifactPath);
  if (artifactBytes.length === 0) throw new Error(`${platform.key}: updater artifact is empty`);
  const url = new URL(encodeURIComponent(artifactName), artifactBaseUrl).toString();
  manifestPlatforms[platform.key] = { signature, url };
  downloadPlatforms[platform.downloadKey] = {
    filename: installerName,
    url: new URL(encodeURIComponent(installerName), artifactBaseUrl).toString(),
  };
  assets.push([artifactPath, artifactName], [join(directory, signatureName), signatureName], [join(directory, installerName), installerName]);
}

await mkdir(outputRoot, { recursive: true });
for (const [source, name] of assets) await copyFile(source, join(outputRoot, name));
await writeFile(
  join(outputRoot, "latest.json"),
  `${JSON.stringify({ version, platforms: manifestPlatforms }, null, 2)}\n`,
  { flag: "wx" },
);
await writeFile(
  join(outputRoot, "downloads.json"),
  `${JSON.stringify({ version, platforms: downloadPlatforms }, null, 2)}\n`,
  { flag: "wx" },
);
console.log(`Assembled signed Tauri updater feed for ${Object.keys(manifestPlatforms).length} platform targets`);

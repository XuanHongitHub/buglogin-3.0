import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function verifyVersionAuthority(root = resolve(import.meta.dirname, "../..")) {
  const rootPackage = JSON.parse(await readFile(resolve(root, "package.json"), "utf8"));
  const packageJson = JSON.parse(await readFile(resolve(root, "apps/desktop/package.json"), "utf8"));
  const tauriConfig = JSON.parse(await readFile(resolve(root, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"));
  const cargoToml = await readFile(resolve(root, "Cargo.toml"), "utf8");
  const cargoLock = await readFile(resolve(root, "Cargo.lock"), "utf8");
  const cargoVersion = workspaceVersion(cargoToml);
  const lockedPackage = tomlPackage(cargoLock, "buglogin-desktop-app", "Cargo.lock");

  requireSemver(packageJson.version, "package.json version");
  requireEqual(cargoVersion, packageJson.version, "Cargo.toml workspace version");
  requireEqual(lockedPackage.version, packageJson.version, "Cargo.lock version");
  requireEqual(tauriConfig.version, packageJson.version, "tauri.conf.json version");
  if (!tauriConfig.bundle?.icon?.includes("icons/icon.icns")) {
    throw new Error("[release-policy] macOS bundle must include icons/icon.icns");
  }
  await access(resolve(root, "apps/desktop/src-tauri/icons/icon.icns"));
  if (!rootPackage.scripts?.tauri?.includes("apps/desktop")) throw new Error("[release-policy] root tauri script must target apps/desktop");
  if (tauriConfig.bundle?.active !== true) throw new Error("[release-policy] production desktop bundling must be active");

  return { version: packageJson.version, productName: tauriConfig.productName, identifier: tauriConfig.identifier };
}

function workspaceVersion(text) {
  const block = text.match(/\[workspace\.package\]([\s\S]*?)(?=\n\[|$)/)?.[1] ?? "";
  const version = block.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error("[release-policy] Cargo.toml missing workspace package version");
  return version;
}

function tomlPackage(text, name, label) {
  for (const block of text.split(/(?=\[\[?package\]?\])/)) {
    const blockName = block.match(/^name\s*=\s*"([^"]+)"/m)?.[1];
    const version = block.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    if (blockName === name && version) return { name: blockName, version };
  }
  throw new Error(`[release-policy] ${label} missing package ${name}`);
}

function requireSemver(value, label) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(value ?? "")) throw new Error(`[release-policy] ${label} is not semantic version: ${value}`);
}

function requireEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`[release-policy] ${label} mismatch: expected ${expected}, found ${actual}`);
}

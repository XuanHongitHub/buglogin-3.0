import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function verifyReleaseWorkflows(root = resolve(import.meta.dirname, "../..")) {
  const release = await readFile(resolve(root, ".github/workflows/release.yml"), "utf8");
  const signing = await readFile(resolve(root, ".github/workflows/signing.yml"), "utf8");
  const platform = await readFile(resolve(root, ".github/workflows/platform-workspace.yml"), "utf8");
  const promotion = await readFile(resolve(root, ".github/workflows/promote-stable.yml"), "utf8");
  verifyReleaseText(release);
  verifySigningText(signing);
  verifyPlatformText(platform);
  verifyPromotionText(promotion);
  return { release, signing, platform, promotion };
}

export function verifyReleaseText(text) {
  requirePatterns(text, [
    /github\.event\.repository\.fork == false/,
    /github\.ref_protected/,
    /persist-credentials:\s*false/,
    /uses:\s*\.\/\.github\/workflows\/signing\.yml/,
    /contents:\s*read/,
    /apps\/desktop\/src-tauri\/tauri\.conf\.json/,
    /publish-rc:\n[\s\S]*?needs:\s*\[validate, sign-and-verify\]/,
    /publish-rc:[\s\S]*?permissions:\n\s+actions:\s*read\n\s+contents:\s*write/,
    /actions\/download-artifact@[0-9a-f]{40}/,
    /verify-bundle\.mjs ["']?\$\{matches\[0\]\}/,
    /gh release create[\s\S]*?--prerelease --verify-tag/,
  ], "release");
  const writePermissionJobs = [...text.matchAll(/^  ([\w-]+):[\s\S]*?^    permissions:\n((?:^      [^\n]*\n)+)/gm)]
    .filter(([, , permissions]) => /^      contents:\s*write$/m.test(permissions))
    .map(([ , job ]) => job);
  if (writePermissionJobs.length !== 1 || writePermissionJobs[0] !== "publish-rc") {
    throw new Error("[release-policy] contents: write must be scoped only to publish-rc");
  }
  forbidPatterns(text, [/pull_request:/, /secrets\./, /zhom\/donutbrowser/i, /deploy|CLOUDFLARE|AWS_SECRET/i, /\.\/src-tauri\/tauri\.conf\.json/], "release");
  verifyPinnedActions(text, "release");
}

export function verifyPromotionText(text) {
  requirePatterns(text, [
    /workflow_dispatch:/,
    /environment:\s*stable-promotion/,
    /contents:\s*write/,
    /gh release download[\s\S]*?SHA256SUMS\.txt/,
    /sha256sum --check SHA256SUMS\.txt/,
    /verify-tauri-updater-feed\.mjs/,
    /gh release edit[\s\S]*?--prerelease=false[\s\S]*?--latest/,
    /github\.ref_name == github\.event\.repository\.default_branch/,
  ], "stable promotion");
  forbidPatterns(text, [/gh release create/, /tauri build|cargo build|pnpm tauri build/i, /--clobber|--delete-asset/], "stable promotion");
  verifyPinnedActions(text, "stable promotion");
}

export function verifyPlatformText(text) {
  requirePatterns(text, [
    /runs-on:\s*\$\{\{ matrix\.runner \}\}/,
    /runner:\s*windows-2025/,
    /runner:\s*macos-14/,
    /runner:\s*macos-15-intel/,
    /tauri-cargo\.mjs test --workspace --all-targets --target/,
    /tauri-cargo\.mjs check -p buglogin-desktop-app --all-targets --target/,
    /pnpm --filter @buglogin\/desktop test/,
    /pnpm --filter @buglogin\/desktop typecheck/,
    /pnpm --filter @buglogin\/desktop build/,
  ], "platform");
  verifyPinnedActions(text, "platform");
  forbidPatterns(text, [/windows-latest|macos-latest|toolchain:\s*stable/, /continue-on-error:\s*true/], "platform");
}

export function verifySigningText(text) {
  requirePatterns(text, [
    /runs-on:\s*windows-2025/,
    /runner:\s*macos-14/,
    /runner:\s*macos-15-intel/,
    /RUST_TOOLCHAIN:\s*1\.95\.0/,
    /environment:\s*release-signing/,
    /node scripts\/release\/verify-runtime-trust-keys\.mjs/,
    /Require Windows signing credentials/,
    /create-tauri-updater-config\.mjs/,
    /TAURI_SIGNING_PRIVATE_KEY/,
    /BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY/,
    /createUpdaterArtifacts = \$true/,
    /nsis\\\.zip/,
    /app\.tar\.gz/,
    /Require Apple signing and notarization credentials/,
    /secrets\.APPLE_CERTIFICATE_P12_BASE64\s*\|\|\s*secrets\.APPLE_CERTIFICATE/,
    /secrets\.APPLE_APP_PASSWORD\s*\|\|\s*secrets\.APPLE_PASSWORD/,
    /Developer ID Application:/,
    /printf 'APPLE_SIGNING_IDENTITY=%s\\n' "\$SELECTED_IDENTITY" >> "\$GITHUB_ENV"/,
    /Get-AuthenticodeSignature/,
    /TimeStamperCertificate/,
    /codesign --verify --deep --strict/,
    /notarytool submit/,
    /stapler validate/,
    /generate-spdx\.mjs/,
    /generate-evidence\.mjs/,
    /verify-bundle\.mjs dist-release(?:\r?\n|$)/,
    /attest-build-provenance@43d14bc2b83dec42d39ecae14e916627a18bb661/,
    /gh attestation verify/,
    /if-no-files-found:\s*error/,
    /if:\s*always\(\)/,
    /target\/\$env:TARGET\/release\/buglogin-desktop-app\.exe/,
    /target\/\$TARGET\/release\/bundle\/macos/,
  ], "signing");
  requireCount(text, /environment:\s*release-signing/g, 2, "signing protected environments");
  requireCount(text, /RUST_TOOLCHAIN:\s*1\.95\.0/g, 2, "signing Rust pins");
  requireCount(text, /node scripts\/release\/verify-runtime-trust-keys\.mjs/g, 2, "embedded runtime trust key validation");
  forbidPatterns(text, [/windows-latest|macos-latest|toolchain:\s*stable/, /gh release|deploy|CLOUDFLARE|aws s3/i, /continue-on-error:\s*true/, /echo .*APPLE_|echo .*WINDOWS_/i, /src-tauri\/target|donutbrowser\.exe|donut-proxy/i, /verify-bundle\.mjs dist-release --require-updater/, /secrets\.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON/], "signing");
  verifyPinnedActions(text, "signing");
}

function verifyPinnedActions(text, label) {
  for (const match of text.matchAll(/uses:\s*([^\s]+)/g)) {
    const reference = match[1];
    if (reference.startsWith("./")) continue;
    if (!/@[0-9a-f]{40}$/.test(reference)) throw new Error(`[release-policy] ${label} action is not pinned: ${reference}`);
  }
}

function requirePatterns(text, patterns, label) {
  for (const pattern of patterns) if (!pattern.test(text)) throw new Error(`[release-policy] ${label} missing ${pattern}`);
}

function requireCount(text, pattern, expected, label) {
  const count = [...text.matchAll(pattern)].length;
  if (count !== expected) throw new Error(`[release-policy] ${label} expected ${expected}, found ${count}`);
}

function forbidPatterns(text, patterns, label) {
  for (const pattern of patterns) if (pattern.test(text)) throw new Error(`[release-policy] ${label} contains forbidden ${pattern}`);
}

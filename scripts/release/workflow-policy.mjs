import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function verifyReleaseWorkflows(root = resolve(import.meta.dirname, "../..")) {
  const release = await readFile(resolve(root, ".github/workflows/release.yml"), "utf8");
  const signing = await readFile(resolve(root, ".github/workflows/signing.yml"), "utf8");
  const platform = await readFile(resolve(root, ".github/workflows/platform-workspace.yml"), "utf8");
  verifyReleaseText(release);
  verifySigningText(signing);
  verifyPlatformText(platform);
  return { release, signing, platform };
}

export function verifyReleaseText(text) {
  requirePatterns(text, [/github\.event\.repository\.fork == false/, /github\.ref_protected/, /persist-credentials:\s*false/, /uses:\s*\.\/\.github\/workflows\/signing\.yml/, /contents:\s*read/, /apps\/desktop\/src-tauri\/tauri\.conf\.json/], "release");
  forbidPatterns(text, [/pull_request:/, /contents:\s*write/, /secrets\./, /zhom\/donutbrowser/i, /gh release|deploy|CLOUDFLARE|AWS_SECRET/i, /\.\/src-tauri\/tauri\.conf\.json/], "release");
  verifyPinnedActions(text, "release");
}

export function verifyPlatformText(text) {
  requirePatterns(text, [
    /runs-on:\s*\$\{\{ matrix\.runner \}\}/,
    /runner:\s*windows-2025/,
    /runner:\s*macos-14/,
    /runner:\s*macos-13/,
    /cargo test --workspace --all-targets --target/,
    /cargo check -p buglogin-desktop-app --all-targets --target/,
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
    /runner:\s*macos-13/,
    /RUST_TOOLCHAIN:\s*1\.95\.0/,
    /environment:\s*release-signing/,
    /Require Windows signing credentials/,
    /Require Apple signing and notarization credentials/,
    /Get-AuthenticodeSignature/,
    /TimeStamperCertificate/,
    /codesign --verify --deep --strict/,
    /notarytool submit/,
    /stapler validate/,
    /generate-spdx\.mjs/,
    /generate-evidence\.mjs/,
    /verify-bundle\.mjs dist-release --require-updater/,
    /attest-build-provenance@43d14bc2b83dec42d39ecae14e916627a18bb661/,
    /gh attestation verify/,
    /if-no-files-found:\s*error/,
    /if:\s*always\(\)/,
    /target\/\$env:TARGET\/release\/buglogin-desktop-app\.exe/,
    /target\/\$TARGET\/release\/bundle\/macos/,
  ], "signing");
  requireCount(text, /environment:\s*release-signing/g, 2, "signing protected environments");
  requireCount(text, /RUST_TOOLCHAIN:\s*1\.95\.0/g, 2, "signing Rust pins");
  forbidPatterns(text, [/windows-latest|macos-latest|toolchain:\s*stable/, /gh release|deploy|CLOUDFLARE|aws s3/i, /continue-on-error:\s*true/, /echo .*APPLE_|echo .*WINDOWS_/i, /src-tauri\/target|donutbrowser\.exe|donut-proxy/i], "signing");
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

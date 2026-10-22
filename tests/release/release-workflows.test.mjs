import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { verifyPlatformText, verifyReleaseText, verifyReleaseWorkflows, verifySigningText } from "../../scripts/release/workflow-policy.mjs";

test("release workflows satisfy fail-closed static policy", async () => {
  const { release, signing, platform, promotion } = await verifyReleaseWorkflows();
  assert.match(release, /workflow_dispatch:/);
  assert.match(signing, /Create Windows updater build configuration/);
  assert.doesNotMatch(signing, /WINDOWS_CERTIFICATE_PFX_BASE64|Get-AuthenticodeSignature/);
  verifyPlatformText(platform);
  assert.match(promotion, /QA_EVIDENCE_URL/);
});

test("OSV dependency scanning does not require private-repository code scanning", async () => {
  const workflow = await readFile(".github/workflows/osv.yml", "utf8");
  assert.equal((workflow.match(/^\s+upload-sarif: false$/gm) ?? []).length, 2);
  assert.match(workflow, /osv-scanner-reusable\.yml@[0-9a-f]{40}/);
  assert.match(workflow, /osv-scanner-reusable-pr\.yml@[0-9a-f]{40}/);
});

test("release publishes verified platform bundles as a downloadable prerelease candidate", async () => {
  const { release } = await verifyReleaseWorkflows();
  assert.match(release, /publish-rc:[\s\S]*?needs:\s*\[validate, sign-and-verify\]/);
  assert.match(release, /publish-rc:[\s\S]*?actions:\s*read\n\s+contents:\s*write/);
  assert.match(release, /actions\/download-artifact@[0-9a-f]{40}/);
  assert.match(release, /find \. -maxdepth 1 -type f ! -name SHA256SUMS\.txt[\s\S]*sha256sum > SHA256SUMS\.txt/);
  assert.match(release, /assemble-tauri-updater-feed\.mjs/);
  assert.match(release, /Publish immutable RC artifacts and feed to DigitalOcean Spaces/);
  assert.match(release, /secrets\.S3_ACCESS_KEY_ID/);
  assert.match(release, /releases\/desktop\/v3\/rc\/latest\.json/);
  const updaterFeed = await readFile("scripts/release/assemble-tauri-updater-feed.mjs", "utf8");
  assert.match(updaterFeed, /latest\.json/);
  assert.match(release, /--prerelease --verify-tag/);
  assert.doesNotMatch(release, /--draft/);
  assert.doesNotMatch(release, /--latest|--make-latest/);
  assert.match(release, /ref: \$\{\{ inputs\.release_ref \}\}/);
  assert.match(release, /workflow_dispatch:[\s\S]*?release_ref:/);
  assert.doesNotMatch(release, /^\s+push:/m);
  assert.match(release, /CURRENT_REF_TYPE" != "branch" \|\| "\$CURRENT_REF" != "\$DEFAULT_BRANCH"/);
  assert.match(release, /release_ref is required/);
  assert.doesNotMatch(release, /git fetch --force --tags/);
});

test("release verifier rejects upstream repository locks and deployment", () => {
  assert.throws(() => verifyReleaseText("github.repository == 'zhom/donutbrowser'\ncontents: write\ndeploy"), /missing|forbidden/);
});

test("stable promotion validates RC assets and changes metadata without rebuilding", async () => {
  const { promotion } = await verifyReleaseWorkflows();
  assert.match(promotion, /environment:\s*stable-promotion/);
  assert.match(promotion, /qa_evidence_url:/);
  assert.match(promotion, /sha256sum --check SHA256SUMS\.txt/);
  assert.match(promotion, /verify-tauri-updater-feed\.mjs/);
  assert.match(promotion, /gh release edit[\s\S]*?--prerelease=false --latest/);
  assert.match(promotion, /Publish stable updater feed to DigitalOcean Spaces/);
  assert.doesNotMatch(promotion, /gh release create|tauri build|cargo build|--clobber/);
  assert.match(promotion, /isDraft\|\|\(!r\.isPrerelease&&!r\.isLatest\)/);
  assert.match(await readFile("scripts/release/verify-tauri-updater-feed.mjs", "utf8"), /stable updater feed requires/);
});

test("stable updater feed binds supported Windows and Apple Silicon Mac assets to the selected release", async () => {
  const directory = await mkdtemp(join(tmpdir(), "buglogin-stable-feed-"));
  try {
    const manifestPath = join(directory, "latest.json");
    const platforms = {
      "windows-x86_64": { signature: "signed", url: "https://github.com/owner/repo/releases/download/v1.2.3/BugLogin_1.2.3_x64-setup.exe" },
      "darwin-aarch64": { signature: "signed", url: "https://github.com/owner/repo/releases/download/v1.2.3/BugLogin_1.2.3_aarch64.app.tar.gz" },
    };
    await writeFile(manifestPath, JSON.stringify({ version: "1.2.3", platforms }));
    run("scripts/release/verify-tauri-updater-feed.mjs", manifestPath, "1.2.3", "owner/repo");
    platforms["windows-x86_64"].url = "https://github.com/owner/repo/releases/download/v1.2.2/BugLogin_1.2.3_x64-setup.exe";
    await writeFile(manifestPath, JSON.stringify({ version: "1.2.3", platforms }));
    assert.throws(() => run("scripts/release/verify-tauri-updater-feed.mjs", manifestPath, "1.2.3", "owner/repo"), /updater URL does not reference/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("stable updater feed validates version-bound Spaces artifact URLs", async () => {
  const directory = await mkdtemp(join(tmpdir(), "buglogin-spaces-feed-"));
  try {
    const manifestPath = join(directory, "latest.json");
    const base = "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/artifacts/";
    const platforms = {
      "windows-x86_64": { signature: "signed", url: `${base}v1.2.3/12345/BugLogin_1.2.3_x64-setup.exe` },
      "darwin-aarch64": { signature: "signed", url: `${base}v1.2.3/12345/BugLogin_1.2.3_aarch64.app.tar.gz` },
    };
    await writeFile(manifestPath, JSON.stringify({ version: "1.2.3", platforms }));
    run("scripts/release/verify-tauri-updater-feed.mjs", manifestPath, "1.2.3", "owner/repo", base);
    platforms["windows-x86_64"].url = `${base}v1.2.2/12345/BugLogin_1.2.3_x64-setup.exe`;
    await writeFile(manifestPath, JSON.stringify({ version: "1.2.3", platforms }));
    assert.throws(() => run("scripts/release/verify-tauri-updater-feed.mjs", manifestPath, "1.2.3", "owner/repo", base));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("signing verifier rejects floating runners and bypassed failures", () => {
  const unsafe = `runs-on: windows-latest\nrunner: macos-latest\ntoolchain: stable\ncontinue-on-error: true`;
  assert.throws(() => verifySigningText(unsafe), /missing|forbidden/);
});

test("signing verifier rejects missing notarization and provenance checks", () => {
  const unsafe = `runs-on: windows-2025\nrunner: macos-14\nRUST_TOOLCHAIN: 1.95.0`;
  assert.throws(() => verifySigningText(unsafe), /missing/);
});

test("Windows release installer does not require Authenticode credentials", async () => {
  const { signing } = await verifyReleaseWorkflows();
  const windows = signing.match(/^  windows:\n([\s\S]*?)(?=^  macos:\n)/m)?.[1];
  assert.ok(windows, "expected Windows build job");
  assert.match(windows, /Build Windows installer and signed updater artifact/);
  assert.match(windows, /Expected Windows app executable and one NSIS installer/);
  assert.match(windows, /Expected the Tauri updater signature for the NSIS installer/);
  assert.doesNotMatch(windows, /WINDOWS_CERTIFICATE|WINDOWS_TIMESTAMP|Authenticode|TimeStamperCertificate/);
});

test("signing workflow maps existing Apple secret names and derives the team identity", async () => {
  const { signing } = await verifyReleaseWorkflows();
  assert.match(signing, /secrets\.APPLE_CERTIFICATE_P12_BASE64\s*\|\|\s*secrets\.APPLE_CERTIFICATE/);
  assert.match(signing, /secrets\.APPLE_APP_PASSWORD\s*\|\|\s*secrets\.APPLE_PASSWORD/);
  assert.match(signing, /Developer ID Application:/);
  assert.match(signing, /APPLE_TEAM_ID/);
  assert.match(signing, /GITHUB_ENV/);
});

test("release signing validates the checked-in public runtime trust set without requiring a secret", async () => {
  const { signing } = await verifyReleaseWorkflows();
  assert.equal((signing.match(/node scripts\/release\/verify-runtime-trust-keys\.mjs/g) ?? []).length, 2);
  assert.doesNotMatch(signing, /secrets\.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON/);
  const windows = signing.match(/^  windows:\n([\s\S]*?)(?=^  macos:\n)/m)?.[1];
  const macos = signing.match(/^  macos:\n([\s\S]*)$/m)?.[1];
  for (const job of [windows, macos]) {
    assert.ok(job, "expected both Windows and macOS signing jobs");
    assert.match(job, /actions\/checkout@[0-9a-f]{40}[\s\S]*?actions\/setup-buglogin[\s\S]*?Validate embedded runtime trust keys[\s\S]*?node scripts\/release\/verify-runtime-trust-keys\.mjs/);
  }
  run("scripts/release/verify-runtime-trust-keys.mjs");
});

test("release verifier rejects mutable action tags", async () => {
  const { release } = await verifyReleaseWorkflows();
  assert.throws(() => verifyReleaseText(release.replace(/actions\/checkout@[0-9a-f]{40}/, "actions/checkout@v6")), /not pinned/);
});

test("release verifier rejects legacy desktop artifact paths", async () => {
  const { signing } = await verifyReleaseWorkflows();
  assert.throws(
    () => verifySigningText(signing.replace("target/$env:TARGET/release/buglogin-desktop-app.exe", "src-tauri/target/$env:TARGET/release/donutbrowser.exe")),
    /missing|forbidden/,
  );
});

test("evidence generators emit SBOM, hashes, and lock-bound provenance", async () => {
  const directory = await mkdtemp(join(tmpdir(), "buglogin-release-"));
  try {
    await writeFile(join(directory, "BugLogin-test.exe"), "artifact", "utf8");
    run("scripts/release/generate-spdx.mjs", join(directory, "dist.spdx.json"));
    run("scripts/release/generate-evidence.mjs", directory, "windows-x64", "v0.28.2", "0123456789abcdef0123456789abcdef01234567", "x86_64-pc-windows-msvc");
    const provenance = JSON.parse(await readFile(join(directory, "provenance.json"), "utf8"));
    assert.deepEqual(provenance.materials.map(({ name }) => name), ["Cargo.lock", "pnpm-lock.yaml", ".github/workflows/release.yml", ".github/workflows/signing.yml"]);
    assert.match(await readFile(join(directory, "SHA256SUMS.txt"), "utf8"), /dist\.spdx\.json/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

function run(script, ...args) {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
}

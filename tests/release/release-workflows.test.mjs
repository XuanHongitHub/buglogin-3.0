import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { verifyPlatformText, verifyReleaseText, verifyReleaseWorkflows, verifySigningText } from "../../scripts/release/workflow-policy.mjs";

test("release workflows satisfy fail-closed static policy", async () => {
  const { release, signing, platform } = await verifyReleaseWorkflows();
  assert.match(release, /workflow_dispatch:/);
  assert.match(signing, /WINDOWS_CERTIFICATE_PFX_BASE64/);
  verifyPlatformText(platform);
});

test("release verifier rejects upstream repository locks and deployment", () => {
  assert.throws(() => verifyReleaseText("github.repository == 'zhom/donutbrowser'\ncontents: write\ndeploy"), /missing|forbidden/);
});

test("signing verifier rejects floating runners and bypassed failures", () => {
  const unsafe = `runs-on: windows-latest\nrunner: macos-latest\ntoolchain: stable\ncontinue-on-error: true`;
  assert.throws(() => verifySigningText(unsafe), /missing|forbidden/);
});

test("signing verifier rejects missing notarization and provenance checks", () => {
  const unsafe = `runs-on: windows-2025\nrunner: macos-14\nrunner: macos-13\nRUST_TOOLCHAIN: 1.95.0`;
  assert.throws(() => verifySigningText(unsafe), /missing/);
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

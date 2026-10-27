import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  tauriCargoEnvironment,
  tauriCargoNeedsDesktopSidecar,
} from "../../scripts/tauri-cargo.mjs";

test("Tauri Cargo keeps build outputs on target and embeds via a relative asset path", () => {
  const config = tauriCargoEnvironment(
    { TAURI_CONFIG: JSON.stringify({ bundle: { active: false } }) },
    "win32",
  );

  assert.equal(config.targetDir, "G:\\buglogin-target");
  assert.equal(config.frontendDist, "../.tauri-cargo-dist-test");
  assert.equal(
    new URL(config.frontendDist, "file:///workspace/src-tauri/").protocol,
    "file:",
  );
  assert.equal(new URL("G:\\buglogin-target\\desktop-dist").protocol, "g:");
  assert.equal(config.env.CARGO_TARGET_DIR, config.targetDir);
  assert.equal(
    JSON.parse(config.env.TAURI_CONFIG).build.frontendDist,
    config.frontendDist,
  );
  assert.equal(JSON.parse(config.env.TAURI_CONFIG).bundle.active, false);
});

test("Tauri release builds stage frontend assets behind a relative directory path", async () => {
  const buildScript = await readFile("scripts/tauri-build.mjs", "utf8");
  const stageScript = await readFile(
    "scripts/stage-tauri-frontend.mjs",
    "utf8",
  );

  assert.match(
    buildScript,
    /frontendStageConfigPath = `\.\.\/\$\{frontendStageName\}`/,
  );
  assert.match(buildScript, /frontendDist: frontendStageConfigPath/);
  assert.match(buildScript, /stage-tauri-frontend\.mjs/);
  assert.match(
    stageScript,
    /Staged Tauri frontend under a relative config path/,
  );
  assert.doesNotMatch(buildScript, /frontendDist:\s*frontendDist/);
});

test("Tauri Cargo rejects forbidden Windows target drives", () => {
  for (const targetDir of [
    "C:\\buglogin-target",
    "E:\\buglogin-target",
    "F:\\buglogin-target",
  ]) {
    assert.throws(
      () => tauriCargoEnvironment({ CARGO_TARGET_DIR: targetDir }, "win32"),
      /single target G:\\buglogin-target/,
    );
  }
});

test("Tauri Cargo rejects malformed existing Tauri config overrides", () => {
  assert.throws(
    () => tauriCargoEnvironment({ TAURI_CONFIG: "not json" }, "win32"),
    /TAURI_CONFIG must be valid JSON/,
  );
});

test("Tauri Cargo stages the migration sidecar only for desktop builds", () => {
  assert.equal(tauriCargoNeedsDesktopSidecar(["test", "--workspace"]), true);
  assert.equal(
    tauriCargoNeedsDesktopSidecar(["check", "-p", "buglogin-desktop-app"]),
    true,
  );
  assert.equal(
    tauriCargoNeedsDesktopSidecar(["test", "-p=buglogin-desktop-app"]),
    true,
  );
  assert.equal(
    tauriCargoNeedsDesktopSidecar(["test", "-p", "buglogin-api"]),
    false,
  );
  assert.equal(
    tauriCargoNeedsDesktopSidecar(["test", "-p", "buglogin-browser"]),
    false,
  );
});

test("Tauri JavaScript plugins match the Rust plugin minor versions", async () => {
  const manifest = JSON.parse(await readFile("package.json", "utf8"));
  const lockfile = await readFile("Cargo.lock", "utf8");

  for (const [jsName, rustName] of [
    ["@tauri-apps/plugin-dialog", "tauri-plugin-dialog"],
    ["@tauri-apps/plugin-fs", "tauri-plugin-fs"],
  ]) {
    const jsVersion = manifest.dependencies[jsName].match(
      /\d+\.\d+\.\d+/,
    )?.[0];
    const rustVersion = lockfile.match(
      new RegExp(
        `\\[\\[package\\]\\]\\nname = "${rustName}"\\nversion = "([^"]+)"`,
      ),
    )?.[1];

    assert.ok(
      jsVersion,
      `missing JavaScript dependency version for ${jsName}`,
    );
    assert.ok(
      rustVersion,
      `missing Rust lockfile version for ${rustName}`,
    );
    assert.equal(
      jsVersion.split(".").slice(0, 2).join("."),
      rustVersion.split(".").slice(0, 2).join("."),
      `${jsName} ${jsVersion} must match ${rustName} ${rustVersion}`,
    );
  }
});

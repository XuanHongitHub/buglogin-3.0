import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";
import {
  tauriCargoEnvironment,
  tauriCargoNeedsDesktopSidecar,
} from "../../scripts/tauri-cargo.mjs";

test("Tauri Cargo keeps frontend assets relative for Windows embedding", () => {
  const config = tauriCargoEnvironment(
    { TAURI_CONFIG: JSON.stringify({ bundle: { active: false } }) },
    "win32",
  );

  assert.equal(config.targetDir, "G:\\buglogin-target");
  assert.equal(config.frontendDist, "../dist");
  assert.equal(config.env.CARGO_TARGET_DIR, config.targetDir);
  assert.equal(
    JSON.parse(config.env.TAURI_CONFIG).build.frontendDist,
    config.frontendDist,
  );
  assert.equal(JSON.parse(config.env.TAURI_CONFIG).bundle.active, false);
});

test("Tauri release build stages target output before relative asset embedding", async () => {
  const buildScript = await readFile(
    resolve(import.meta.dirname, "../../scripts/tauri-build.mjs"),
    "utf8",
  );
  assert.match(
    buildScript,
    /await cp\(frontendDist, frontendBundleStage, \{ recursive: true \}\)/,
  );
  assert.match(
    buildScript,
    /frontendDist: "\.\.\/dist", beforeBuildCommand: ""/,
  );
  assert.doesNotMatch(
    buildScript,
    /frontendDist, beforeBuildCommand: "pnpm build"/,
  );
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

import assert from "node:assert/strict";
import test from "node:test";
import {
  tauriCargoEnvironment,
  tauriCargoNeedsDesktopSidecar,
} from "../../scripts/tauri-cargo.mjs";

test("Tauri Cargo uses the selected build target for frontend assets", () => {
  const config = tauriCargoEnvironment(
    { TAURI_CONFIG: JSON.stringify({ bundle: { active: false } }) },
    "win32",
  );

  assert.equal(config.targetDir, "E:\\buglogin-target");
  assert.equal(config.frontendDist, "E:\\buglogin-target\\desktop-dist");
  assert.equal(config.env.CARGO_TARGET_DIR, config.targetDir);
  assert.equal(
    JSON.parse(config.env.TAURI_CONFIG).build.frontendDist,
    config.frontendDist,
  );
  assert.equal(JSON.parse(config.env.TAURI_CONFIG).bundle.active, false);
});

test("Tauri Cargo rejects forbidden Windows target drives", () => {
  for (const targetDir of ["C:\\buglogin-target", "F:\\buglogin-target"]) {
    assert.throws(
      () => tauriCargoEnvironment({ CARGO_TARGET_DIR: targetDir }, "win32"),
      /single target E:\\buglogin-target/,
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

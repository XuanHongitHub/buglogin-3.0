import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { EventEmitter } from "node:events";
import {
  lstat,
  mkdir,
  readdir,
  readFile,
  readlink,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";
import { resolveBuildTargetDir } from "../../scripts/build-target.mjs";
import {
  forwardTerminationSignals,
  recoverTauriSidecarDirectory,
  stageTauriSidecarDirectory,
} from "../../scripts/tauri-sidecar-staging.mjs";

const sidecarName = `buglogin-migrate-${process.platform === "win32" ? "x64" : "test"}${process.platform === "win32" ? ".exe" : ""}`;

async function withTempRoot(run) {
  const root = resolve(
    resolveBuildTargetDir(),
    `verification-tauri-sidecar-${randomUUID()}`,
  );
  await mkdir(root, { recursive: true });
  try {
    await run(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test("Tauri build staging restores the original binaries directory and sidecar", async () => {
  await withTempRoot(async (root) => {
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    const sidecarStage = resolve(root, "target", "sidecars");
    await mkdir(sourceBinaries, { recursive: true });
    await mkdir(sidecarStage, { recursive: true });
    await writeFile(resolve(sourceBinaries, sidecarName), "dev-sidecar");
    await writeFile(resolve(sidecarStage, sidecarName), "release-sidecar");

    const restore = await stageTauriSidecarDirectory({
      sourceBinaries,
      sidecarStage,
      sidecarName,
    });
    assert.equal((await lstat(sourceBinaries)).isDirectory(), true);
    assert.deepEqual(await readdir(sourceBinaries), [sidecarName]);
    assert.equal(
      await readFile(resolve(sourceBinaries, sidecarName), "utf8"),
      "release-sidecar",
    );

    await restore();
    await restore();
    assert.equal((await lstat(sourceBinaries)).isDirectory(), true);
    assert.equal(
      await readFile(resolve(sourceBinaries, sidecarName), "utf8"),
      "dev-sidecar",
    );
  });
});

test("Tauri build staging restores a development junction without changing its target", async () => {
  await withTempRoot(async (root) => {
    const devTarget = resolve(root, "target", "dev");
    const sidecarStage = resolve(root, "target", "release");
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    await mkdir(devTarget, { recursive: true });
    await mkdir(sidecarStage, { recursive: true });
    await mkdir(resolve(root, "src-tauri"), { recursive: true });
    await writeFile(resolve(devTarget, sidecarName), "dev-sidecar");
    await writeFile(resolve(sidecarStage, sidecarName), "release-sidecar");
    await symlink(
      devTarget,
      sourceBinaries,
      process.platform === "win32" ? "junction" : "dir",
    );

    const originalTarget = await readlink(sourceBinaries);
    const restore = await stageTauriSidecarDirectory({
      sourceBinaries,
      sidecarStage,
      sidecarName,
    });
    assert.equal((await lstat(sourceBinaries)).isDirectory(), true);
    await restore();

    assert.equal(await readlink(sourceBinaries), originalTarget);
    assert.equal(
      await readFile(resolve(devTarget, sidecarName), "utf8"),
      "dev-sidecar",
    );
  });
});

test("Tauri build staging restores the original junction if the bundler replaces the temporary directory", async () => {
  await withTempRoot(async (root) => {
    const devTarget = resolve(root, "target", "dev");
    const sidecarStage = resolve(root, "target", "release");
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    await mkdir(devTarget, { recursive: true });
    await mkdir(sidecarStage, { recursive: true });
    await mkdir(resolve(root, "src-tauri"), { recursive: true });
    await writeFile(resolve(devTarget, sidecarName), "dev-sidecar");
    await writeFile(resolve(sidecarStage, sidecarName), "release-sidecar");
    await symlink(
      devTarget,
      sourceBinaries,
      process.platform === "win32" ? "junction" : "dir",
    );

    const originalTarget = await readlink(sourceBinaries);
    const restore = await stageTauriSidecarDirectory({
      sourceBinaries,
      sidecarStage,
      sidecarName,
    });
    await rm(sourceBinaries, { recursive: true, force: true });
    await mkdir(sourceBinaries, { recursive: true });
    await writeFile(resolve(sourceBinaries, ".gitkeep"), "");

    await restore();
    assert.equal(await readlink(sourceBinaries), originalTarget);
    assert.equal(
      await readFile(resolve(devTarget, sidecarName), "utf8"),
      "dev-sidecar",
    );
  });
});

test("Tauri build staging refuses unknown binaries without moving the original directory", async () => {
  await withTempRoot(async (root) => {
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    await mkdir(sourceBinaries, { recursive: true });
    await writeFile(resolve(sourceBinaries, "unexpected.bin"), "keep");

    await assert.rejects(
      stageTauriSidecarDirectory({
        sourceBinaries,
        sidecarStage: resolve(root, "target", "release"),
        sidecarName,
      }),
      /Refusing to replace unexpected Tauri binaries entries/,
    );
    assert.equal(
      await readdir(sourceBinaries).then((entries) => entries[0]),
      "unexpected.bin",
    );
  });
});

test("Tauri staging recovers a backup left by an interrupted launcher", async () => {
  await withTempRoot(async (root) => {
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    const backup = `${sourceBinaries}.build-restore-${randomUUID()}`;
    await mkdir(sourceBinaries, { recursive: true });
    await mkdir(backup, { recursive: true });
    await writeFile(resolve(sourceBinaries, sidecarName), "staged-sidecar");
    await writeFile(resolve(backup, ".gitkeep"), "");

    assert.equal(
      await recoverTauriSidecarDirectory({ sourceBinaries, sidecarName }),
      true,
    );
    assert.deepEqual(await readdir(sourceBinaries), [".gitkeep"]);
    assert.deepEqual((await readdir(resolve(root, "src-tauri"))).sort(), [
      "binaries",
    ]);
    assert.equal(
      await recoverTauriSidecarDirectory({ sourceBinaries, sidecarName }),
      false,
    );
  });
});

test("Tauri staging refuses to guess between several leftover backups", async () => {
  await withTempRoot(async (root) => {
    const sourceBinaries = resolve(root, "src-tauri", "binaries");
    await mkdir(sourceBinaries, { recursive: true });
    await mkdir(`${sourceBinaries}.build-restore-${randomUUID()}`);
    await mkdir(`${sourceBinaries}.build-restore-${randomUUID()}`);

    await assert.rejects(
      recoverTauriSidecarDirectory({ sourceBinaries, sidecarName }),
      /Multiple Tauri binaries backups/,
    );
  });
});

test("Tauri dev launcher forwards termination signals to the child instead of dying", () => {
  const launcher = new EventEmitter();
  const forwarded = [];
  const child = { exitCode: null, kill: (signal) => forwarded.push(signal) };

  forwardTerminationSignals(child, launcher);
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    assert.equal(launcher.listenerCount(signal), 1, signal);
    launcher.emit(signal, signal);
  }
  assert.deepEqual(forwarded, ["SIGINT", "SIGTERM", "SIGHUP"]);

  child.exitCode = 0;
  launcher.emit("SIGTERM", "SIGTERM");
  assert.deepEqual(forwarded, ["SIGINT", "SIGTERM", "SIGHUP"]);
});

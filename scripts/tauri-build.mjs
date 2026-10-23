#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, mkdir, rm } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "./build-target.mjs";
import { stageTauriSidecarDirectory } from "./tauri-sidecar-staging.mjs";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const desktopRoot = resolve(repositoryRoot, "apps/desktop");
const cargoTargetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const frontendDist = resolve(cargoTargetDir, "desktop-dist");
const frontendBundleStage = resolve(desktopRoot, "dist");
const targetArgumentIndex = process.argv.indexOf("--target");
const targetArgument = process.argv.find((argument) =>
  argument.startsWith("--target="),
);
const targetTriple =
  targetArgument?.slice("--target=".length) ??
  (targetArgumentIndex >= 0
    ? process.argv[targetArgumentIndex + 1]
    : undefined) ??
  process.env.TAURI_ENV_TARGET_TRIPLE ??
  process.env.RUST_TARGET ??
  process.env.TARGET ??
  execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
    /^host: (.+)$/m,
  )?.[1];
if (!targetTriple) throw new Error("Rust target triple unavailable");
const targetRoot = resolve(cargoTargetDir, targetTriple, "release");
const debugBuild = process.argv.includes("--debug");
const noBundle = process.argv.includes("--no-bundle");
const sidecarSuffix = process.platform === "win32" ? ".exe" : "";
const sidecarName = `buglogin-migrate-${targetTriple}${sidecarSuffix}`;
const sidecarSource = resolve(targetRoot, `buglogin-migrate${sidecarSuffix}`);
const sidecarStage = resolve(cargoTargetDir, "tauri-sidecars");
const sidecarStaged = resolve(sidecarStage, sidecarName);
const sourceBinaries = resolve(desktopRoot, "src-tauri/binaries");
const childEnv = {
  ...process.env,
  CARGO_TARGET_DIR: cargoTargetDir,
  TAURI_ENV_TARGET_TRIPLE: targetTriple,
};
let restoreBinariesDirectory;

async function stageFrontendBundle() {
  execFileSync("pnpm", ["build"], {
    cwd: desktopRoot,
    env: childEnv,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (!existsSync(resolve(frontendDist, "index.html"))) {
    throw new Error(
      `Frontend build did not produce ${resolve(frontendDist, "index.html")}`,
    );
  }
  await rm(frontendBundleStage, { recursive: true, force: true });
  await cp(frontendDist, frontendBundleStage, { recursive: true });
}

async function cleanupFrontendBundleStage() {
  const expectedStage = resolve(desktopRoot, "dist");
  if (frontendBundleStage !== expectedStage) {
    throw new Error(
      `Refusing to remove unexpected frontend staging path: ${frontendBundleStage}`,
    );
  }
  await rm(frontendBundleStage, { recursive: true, force: true });
}

if (!(debugBuild && noBundle)) {
  execFileSync(
    process.execPath,
    [resolve(desktopRoot, "copy-migration-binary.mjs")],
    {
      cwd: desktopRoot,
      env: childEnv,
      stdio: "inherit",
    },
  );
  await mkdir(sidecarStage, { recursive: true });
  await cp(sidecarSource, sidecarStaged, { force: true });

  restoreBinariesDirectory = await stageTauriSidecarDirectory({
    sourceBinaries,
    sidecarStage,
    sidecarName,
  });
}

try {
  await stageFrontendBundle();
} catch (error) {
  await cleanupFrontendBundleStage();
  await restoreBinariesDirectory?.();
  throw error;
}

const child = spawn(
  process.execPath,
  [
    resolve(repositoryRoot, "node_modules/@tauri-apps/cli/tauri.js"),
    "build",
    "--config",
    "src-tauri/tauri.conf.json",
    "--config",
    JSON.stringify({
      build: { frontendDist: "../dist", beforeBuildCommand: "" },
      ...(debugBuild && noBundle ? { bundle: { externalBin: [] } } : {}),
    }),
    ...process.argv.slice(2),
  ],
  { cwd: desktopRoot, env: childEnv, stdio: "inherit" },
);

async function finishBuild(code, signal) {
  try {
    await cleanupFrontendBundleStage();
    await restoreBinariesDirectory?.();
  } catch (error) {
    console.error(
      `Could not restore Tauri binaries directory: ${error.message}`,
    );
    process.exitCode = 1;
    return;
  }
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
}

child.on("error", async (error) => {
  console.error(`Could not start BugLogin Tauri build: ${error.message}`);
  await finishBuild(1);
});
child.on("exit", (code, signal) => {
  void finishBuild(code, signal);
});

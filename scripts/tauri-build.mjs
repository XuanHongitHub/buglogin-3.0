#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { rm } from "node:fs/promises";
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "./build-target.mjs";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const desktopRoot = resolve(repositoryRoot, "apps/desktop");
const cargoTargetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const frontendStageName = `.tauri-frontend-dist-${randomUUID()}`;
const frontendStagePath = resolve(desktopRoot, frontendStageName);
const frontendStageConfigPath = `../${frontendStageName}`;
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
const childEnv = {
  ...process.env,
  CARGO_TARGET_DIR: cargoTargetDir,
  BUGLOGIN_TAURI_FRONTEND_DIST_STAGE: frontendStagePath,
  TAURI_ENV_TARGET_TRIPLE: targetTriple,
};

const child = spawn(
  process.execPath,
  [
    resolve(repositoryRoot, "node_modules/@tauri-apps/cli/tauri.js"),
    "build",
    "--config",
    "src-tauri/tauri.conf.json",
    "--config",
    JSON.stringify({
      build: {
        // Tauri interprets a Windows drive path such as G:\\... as a URI.
        // Use a relative directory so Tauri embeds frontend assets in the app.
        frontendDist: frontendStageConfigPath,
        beforeBuildCommand:
          "pnpm build && node ../../scripts/stage-tauri-frontend.mjs",
      },
      ...(debugBuild && noBundle ? { bundle: { externalBin: [] } } : {}),
    }),
    ...process.argv.slice(2),
  ],
  { cwd: desktopRoot, env: childEnv, stdio: "inherit" },
);

async function finishBuild(code, signal) {
  try {
    const stagedPath = relative(desktopRoot, frontendStagePath);
    if (
      !stagedPath ||
      stagedPath === ".." ||
      stagedPath.startsWith(`..${sep}`)
    ) {
      throw new Error(
        "Refusing to remove frontend staging outside desktop root",
      );
    }
    await rm(frontendStagePath, { recursive: true, force: true });
  } catch (error) {
    console.error(`Could not clean Tauri build staging: ${error.message}`);
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

#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { cp, mkdir, rm } from "node:fs/promises";
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "./build-target.mjs";
import { stageTauriSidecarDirectory } from "./tauri-sidecar-staging.mjs";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));

export function tauriCargoEnvironment(
  environment = process.env,
  platform = process.platform,
  frontendStageName = ".tauri-cargo-dist-test",
) {
  const targetDir = resolveBuildTargetDir(
    environment.CARGO_TARGET_DIR,
    platform,
  );
  if (!/^\.tauri-cargo-dist-[A-Za-z0-9-]+$/.test(frontendStageName)) {
    throw new Error("Tauri Cargo frontend stage name is invalid");
  }
  const frontendDist = `../${frontendStageName}`;
  let tauriConfig = {};
  const configured = environment.TAURI_CONFIG?.trim();
  if (configured) {
    try {
      tauriConfig = JSON.parse(configured);
    } catch {
      throw new Error("TAURI_CONFIG must be valid JSON when set");
    }
    if (
      !tauriConfig ||
      typeof tauriConfig !== "object" ||
      Array.isArray(tauriConfig)
    ) {
      throw new Error("TAURI_CONFIG must be a JSON object when set");
    }
  }
  return {
    targetDir,
    frontendDist,
    env: {
      ...environment,
      CARGO_TARGET_DIR: targetDir,
      TAURI_CONFIG: JSON.stringify({
        ...tauriConfig,
        build: { ...tauriConfig.build, frontendDist },
      }),
    },
  };
}

export function tauriCargoNeedsDesktopSidecar(args) {
  if (args.includes("--workspace")) return true;
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "-p" || argument === "--package") {
      if (args[index + 1] === "buglogin-desktop-app") return true;
    }
    if (
      argument === "-p=buglogin-desktop-app" ||
      argument === "--package=buglogin-desktop-app"
    ) {
      return true;
    }
  }
  return false;
}

function targetTripleFrom(args, environment) {
  const targetIndex = args.indexOf("--target");
  const targetFlag = args.find((argument) => argument.startsWith("--target="));
  return (
    targetFlag?.slice("--target=".length) ??
    (targetIndex >= 0 ? args[targetIndex + 1] : undefined) ??
    environment.TAURI_ENV_TARGET_TRIPLE ??
    environment.RUST_TARGET ??
    environment.TARGET ??
    execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
      /^host: (.+)$/m,
    )?.[1]
  );
}

async function stageDesktopMigrationSidecar(args, config) {
  if (!tauriCargoNeedsDesktopSidecar(args)) return async () => {};

  const targetTriple = targetTripleFrom(args, process.env);
  if (!targetTriple) throw new Error("Rust target triple unavailable");

  const suffix = targetTriple.includes("windows") ? ".exe" : "";
  const sidecarName = `buglogin-migrate-${targetTriple}${suffix}`;
  const targetScoped =
    args.includes("--target") ||
    args.some((argument) => argument.startsWith("--target=")) ||
    Boolean(
      process.env.TAURI_ENV_TARGET_TRIPLE ||
        process.env.RUST_TARGET ||
        process.env.TARGET,
    );
  const cargoTargetRoot = resolve(
    config.targetDir,
    ...(targetScoped ? [targetTriple] : []),
    "debug",
  );
  const sidecarSource = resolve(cargoTargetRoot, `buglogin-migrate${suffix}`);
  const sidecarStage = resolve(config.targetDir, "tauri-sidecars", "cargo");
  const sidecarStaged = resolve(sidecarStage, sidecarName);
  const sourceBinaries = resolve(
    repositoryRoot,
    "apps/desktop/src-tauri/binaries",
  );
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  const buildArgs = [
    "build",
    "--locked",
    "-p",
    "buglogin-migration",
    "--bin",
    "buglogin-migrate",
  ];
  if (targetScoped) buildArgs.push("--target", targetTriple);

  execFileSync(cargo, buildArgs, {
    cwd: repositoryRoot,
    env: { ...config.env, TAURI_ENV_TARGET_TRIPLE: targetTriple },
    stdio: "inherit",
  });
  await mkdir(sidecarStage, { recursive: true });
  await cp(sidecarSource, sidecarStaged, { force: true });
  return stageTauriSidecarDirectory({
    sourceBinaries,
    sidecarStage,
    sidecarName,
  });
}

async function main(args) {
  if (args.length === 0 || args.includes("--help")) {
    console.log("Usage: node scripts/tauri-cargo.mjs <cargo arguments>");
    return;
  }

  const frontendStageName = `.tauri-cargo-dist-${randomUUID()}`;
  const config = tauriCargoEnvironment(
    process.env,
    process.platform,
    frontendStageName,
  );
  const frontendStagePath = resolve(
    repositoryRoot,
    "apps/desktop",
    frontendStageName,
  );
  const relativeStagePath = relative(
    resolve(repositoryRoot, "apps/desktop"),
    frontendStagePath,
  );
  if (
    !relativeStagePath ||
    relativeStagePath === ".." ||
    relativeStagePath.startsWith(`..${sep}`)
  ) {
    throw new Error(
      "Refusing to stage Tauri Cargo frontend outside desktop root",
    );
  }
  await mkdir(frontendStagePath);
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  let restoreSidecar = async () => {};
  let terminationSignal;
  try {
    restoreSidecar = await stageDesktopMigrationSidecar(args, config);
    const child = spawn(cargo, args, {
      cwd: repositoryRoot,
      env: config.env,
      stdio: "inherit",
    });
    const result = await new Promise((resolveResult, rejectResult) => {
      child.on("error", rejectResult);
      child.on("exit", (code, signal) => resolveResult({ code, signal }));
    });
    if (result.signal) terminationSignal = result.signal;
    else process.exitCode = result.code ?? 1;
  } catch (error) {
    console.error(`Could not run BugLogin Cargo check: ${error.message}`);
    process.exitCode = 1;
  } finally {
    try {
      await restoreSidecar();
    } catch (error) {
      console.error(
        `Could not restore Tauri binaries directory: ${error.message}`,
      );
      process.exitCode = 1;
    }
    try {
      await rm(frontendStagePath, { recursive: true, force: true });
    } catch (error) {
      console.error(
        `Could not remove Tauri Cargo frontend stage: ${error.message}`,
      );
      process.exitCode = 1;
    }
  }
  if (terminationSignal) process.kill(process.pid, terminationSignal);
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  await main(process.argv.slice(2));
}

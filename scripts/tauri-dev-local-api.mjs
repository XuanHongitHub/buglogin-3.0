#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { access, cp, mkdir, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { resolveBuildTargetDir } from "./build-target.mjs";
import {
  findMissingTauriDevOperations,
  hasDurableTauriDevPersistence,
  isForbiddenTauriDevCargoTargetDir,
  parseTauriDevApiBaseUrl,
  validateMcpStatusResponse,
} from "./tauri-dev-local-api-contract.mjs";
import {
  forwardTerminationSignals,
  recoverTauriSidecarDirectory,
  stageTauriSidecarDirectory,
} from "./tauri-sidecar-staging.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = resolve(repositoryRoot, "apps/desktop");
const localEnv = new Map();
try {
  const contents = await readFile(resolve(repositoryRoot, ".env.local"), "utf8");
  for (const line of contents.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = trimmed.indexOf("=");
    if (separator <= 0) throw new Error(".env.local contains an invalid entry");
    localEnv.set(trimmed.slice(0, separator).trim(), trimmed.slice(separator + 1));
  }
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
}
const apiBaseUrl = parseTauriDevApiBaseUrl(
  process.env.BUGLOGIN_DEV_API_BASE_URL ?? "http://127.0.0.1:12345",
);
const cargoTargetDir = resolveBuildTargetDir(
  process.env.BUGLOGIN_DEV_CARGO_TARGET_DIR,
);
if (isForbiddenTauriDevCargoTargetDir(cargoTargetDir)) {
  throw new Error("BugLogin development builds cannot use the C: or F: drive");
}

async function waitForLocalApi() {
  const correlationId = `desktop-dev-${randomUUID()}`;
  let lastError = "API did not respond";

  for (let attempt = 0; attempt < 10; attempt += 1) {
    let retry = true;
    try {
      const response = await fetch(`${apiBaseUrl}/v1/readyz`, {
        headers: { "x-correlation-id": correlationId },
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) {
        const body = await response.json();
        const persistenceMode = response.headers.get(
          "x-buglogin-persistence-mode",
        );
        if (
          body.status === "ready" &&
          body.service === "buglogin-api" &&
          hasDurableTauriDevPersistence(persistenceMode)
        ) {
          const openApiResponse = await fetch(`${apiBaseUrl}/v1/openapi.yaml`, {
            headers: { "x-correlation-id": correlationId },
            signal: AbortSignal.timeout(1_000),
          });
          if (!openApiResponse.ok) {
            lastError = `Local API OpenAPI returned HTTP ${openApiResponse.status}`;
            retry = false;
          } else {
            let openApi;
            try {
              openApi = parse(await openApiResponse.text());
            } catch {
              lastError = "Local BugLogin API returned an invalid OpenAPI document";
              retry = false;
            }
            if (retry) {
              const missing = findMissingTauriDevOperations(openApi);
              if (missing.length > 0) {
                lastError = `Local BugLogin API is missing desktop contract operations: ${missing.join(", ")}`;
                retry = false;
              }
            }
          }
          if (retry) {
            const mcpStatusResponse = await fetch(`${apiBaseUrl}/v1/mcp/status`, {
              headers: { "x-correlation-id": correlationId },
              signal: AbortSignal.timeout(1_000),
            });
            if (!mcpStatusResponse.ok) {
              lastError = `Local BugLogin API MCP status returned HTTP ${mcpStatusResponse.status}`;
              retry = false;
            } else {
              let mcpStatus;
              try {
                mcpStatus = await mcpStatusResponse.json();
              } catch {
                lastError = "Local BugLogin API returned invalid MCP status JSON";
                retry = false;
              }
              if (retry && !validateMcpStatusResponse(mcpStatus)) {
                lastError = "Local BugLogin API returned an invalid MCP status contract";
                retry = false;
              }
            }
          }
          if (retry) return;
        } else if (body.status === "ready" && body.service === "buglogin-api") {
          lastError = `Local BugLogin API persistence is "${persistenceMode ?? "unknown"}"; desktop dev requires durable SQLite or PostgreSQL persistence`;
          retry = false;
        } else {
          lastError = "The local endpoint is not the BugLogin API";
          retry = false;
        }
      } else {
        lastError = `Local API readiness returned HTTP ${response.status}`;
      }
    } catch (error) {
      lastError =
        error instanceof Error ? error.message : "API did not respond";
    }
    if (!retry) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`${lastError}. Expected ${apiBaseUrl}/v1/readyz`);
}

await waitForLocalApi();
console.log(`BugLogin desktop will use local API ${apiBaseUrl}.`);

if (process.argv.includes("--check")) process.exit(0);

const targetTriple =
  process.env.TAURI_ENV_TARGET_TRIPLE ??
  execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)?.[1];
if (!targetTriple) throw new Error("Rust target triple unavailable");
const sidecarSuffix = process.platform === "win32" ? ".exe" : "";
const sidecarName = `buglogin-migrate-${targetTriple}${sidecarSuffix}`;
const sidecarSource = resolve(
  cargoTargetDir,
  ...(process.env.TAURI_ENV_TARGET_TRIPLE ? [targetTriple] : []),
  "debug",
  `buglogin-migrate${sidecarSuffix}`,
);
const sidecarStage = resolve(cargoTargetDir, "tauri-sidecars", "dev");
const sourceBinaries = resolve(desktopRoot, "src-tauri/binaries");
let restoreStagedBinaries;

async function restoreBinariesDirectory() {
  await restoreStagedBinaries?.();
  restoreStagedBinaries = undefined;
}

async function stageMigrationSidecar() {
  const sidecarEnv = { ...process.env, CARGO_TARGET_DIR: cargoTargetDir };
  try {
    await access(sidecarSource);
  } catch {
    const cargoArgs = [
      "build",
      "--locked",
      "-p",
      "buglogin-migration",
      "--bin",
      "buglogin-migrate",
    ];
    if (process.env.TAURI_ENV_TARGET_TRIPLE) cargoArgs.push("--target", targetTriple);
    execFileSync(process.platform === "win32" ? "cargo.exe" : "cargo", cargoArgs, {
      cwd: repositoryRoot,
      env: sidecarEnv,
      stdio: "inherit",
    });
    await access(sidecarSource);
  }
  await mkdir(sidecarStage, { recursive: true });
  await cp(sidecarSource, resolve(sidecarStage, sidecarName), { force: true });

  restoreStagedBinaries = await stageTauriSidecarDirectory({
    sourceBinaries,
    sidecarStage,
    sidecarName,
  });
}

const childEnv = { ...process.env };
childEnv.BUGLOGIN_DEPLOYMENT_MODE = "production";
childEnv.BUGLOGIN_API_BASE_URL = apiBaseUrl;
childEnv.CARGO_TARGET_DIR = cargoTargetDir;
const runtimeTrustKeysJson =
  process.env.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON?.trim() ||
  localEnv.get("BUGLOGIN_RUNTIME_TRUST_KEYS_JSON")?.trim();
if (runtimeTrustKeysJson) {
  let runtimeTrustKeys;
  try {
    runtimeTrustKeys = JSON.parse(runtimeTrustKeysJson);
  } catch {
    throw new Error("BUGLOGIN_RUNTIME_TRUST_KEYS_JSON must be valid JSON");
  }
  if (
    !runtimeTrustKeys ||
    typeof runtimeTrustKeys !== "object" ||
    Array.isArray(runtimeTrustKeys) ||
    Object.keys(runtimeTrustKeys).length === 0 ||
    Object.values(runtimeTrustKeys).some(
      (key) => typeof key !== "string" || !/^[0-9a-f]{64}$/i.test(key),
    )
  ) {
    throw new Error("BUGLOGIN_RUNTIME_TRUST_KEYS_JSON must map key IDs to Ed25519 public keys");
  }
  childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON = JSON.stringify(runtimeTrustKeys);
} else {
  delete childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON;
}
delete childEnv.BUGLOGIN_API_TOKEN;
delete childEnv.BUGLOGIN_DATA_ROOT;
delete childEnv.BUGLOGIN_PROFILE_CACHE_ROOT;

await recoverTauriSidecarDirectory({ sourceBinaries, sidecarName });
await stageMigrationSidecar();

const isWindows = process.platform === "win32";
const child = spawn(
  isWindows ? "pnpm.cmd" : "pnpm",
  [
    "exec",
    "tauri",
    "dev",
    "--config",
    "src-tauri/tauri.conf.json",
    ...process.argv.slice(2),
  ],
  {
    cwd: desktopRoot,
    env: childEnv,
    shell: isWindows,
    stdio: "inherit",
  },
);

child.on("error", (error) => {
  console.error(`Could not start BugLogin desktop: ${error.message}`);
  void restoreBinariesDirectory().finally(() => {
    process.exitCode = 1;
  });
});
forwardTerminationSignals(child);
child.on("exit", (code, signal) => {
  void restoreBinariesDirectory().finally(() => {
    process.exitCode = signal ? 130 : (code ?? 1);
  });
});

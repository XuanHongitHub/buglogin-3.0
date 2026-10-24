#!/usr/bin/env node

import { spawn, execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "./build-target.mjs";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const localEnvPath = resolve(repositoryRoot, ".env.local");
const localEnv = new Map();
for (const line of (await readFile(localEnvPath, "utf8")).split(/\r?\n/)) {
  const trimmed = line.trim();
  if (!trimmed || trimmed.startsWith("#")) continue;
  const separator = trimmed.indexOf("=");
  if (separator <= 0) throw new Error(".env.local contains an invalid entry");
  localEnv.set(trimmed.slice(0, separator).trim(), trimmed.slice(separator + 1));
}

const databaseUrl = localEnv.get("BUGLOGIN_POSTGRES_URL");
if (!databaseUrl) throw new Error(".env.local is missing BUGLOGIN_POSTGRES_URL");
const localEnvValue = (name) =>
  process.env[name]?.trim() || localEnv.get(name)?.trim() || undefined;
const runtimeCatalogJson = localEnvValue("BUGLOGIN_RUNTIME_CATALOG_JSON");
const runtimeTrustKeysJson = localEnvValue("BUGLOGIN_RUNTIME_TRUST_KEYS_JSON");
const runtimeCatalogPath = localEnvValue("BUGLOGIN_RUNTIME_CATALOG_PATH");
const runtimeTrustKeysPath = localEnvValue("BUGLOGIN_RUNTIME_TRUST_KEYS_PATH");
const hasInlineRuntimeConfig = Boolean(runtimeCatalogJson || runtimeTrustKeysJson);
const hasPathRuntimeConfig = Boolean(runtimeCatalogPath || runtimeTrustKeysPath);
if (hasInlineRuntimeConfig && hasPathRuntimeConfig) {
  throw new Error("Local runtime config must use inline JSON or file paths, not both");
}
if (hasInlineRuntimeConfig) {
  if (!runtimeCatalogJson || !runtimeTrustKeysJson) {
    throw new Error(
      "Local inline runtime config requires both BUGLOGIN_RUNTIME_CATALOG_JSON and BUGLOGIN_RUNTIME_TRUST_KEYS_JSON",
    );
  }
  if (Buffer.byteLength(runtimeCatalogJson, "utf8") > 24 * 1024) {
    throw new Error("BUGLOGIN_RUNTIME_CATALOG_JSON exceeds the local environment limit");
  }
  let catalog;
  let trustKeys;
  try {
    catalog = JSON.parse(runtimeCatalogJson);
    trustKeys = JSON.parse(runtimeTrustKeysJson);
  } catch {
    throw new Error("Local inline runtime config must contain valid JSON");
  }
  if (
    !catalog ||
    typeof catalog !== "object" ||
    !Array.isArray(catalog.releases) ||
    !trustKeys ||
    typeof trustKeys !== "object" ||
    Array.isArray(trustKeys) ||
    Object.keys(trustKeys).length === 0 ||
    Object.values(trustKeys).some(
      (key) => typeof key !== "string" || !/^[0-9a-f]{64}$/i.test(key),
    )
  ) {
    throw new Error("Local inline runtime catalog or public trust map is invalid");
  }
} else if (Boolean(runtimeCatalogPath) !== Boolean(runtimeTrustKeysPath)) {
  throw new Error(
    "Local file-based runtime config requires both BUGLOGIN_RUNTIME_CATALOG_PATH and BUGLOGIN_RUNTIME_TRUST_KEYS_PATH",
  );
}
for (const [name, value] of [
  ["BUGLOGIN_RUNTIME_CATALOG_PATH", runtimeCatalogPath],
  ["BUGLOGIN_RUNTIME_TRUST_KEYS_PATH", runtimeTrustKeysPath],
]) {
  if (value && !isAbsolute(value)) {
    throw new Error(`Local ${name} must be an absolute path`);
  }
}
let parsedDatabaseUrl;
try {
  parsedDatabaseUrl = new URL(databaseUrl);
} catch {
  throw new Error(".env.local has an invalid PostgreSQL URL");
}
if (
  !["postgres:", "postgresql:"].includes(parsedDatabaseUrl.protocol) ||
  !["localhost", "127.0.0.1", "[::1]"].includes(parsedDatabaseUrl.hostname) ||
  parsedDatabaseUrl.searchParams.get("sslmode") !== "require"
) {
  throw new Error("Local API requires loopback PostgreSQL with sslmode=require");
}

const cargoTargetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const cargoPath = execFileSync(
  process.platform === "win32" ? "where.exe" : "which",
  [cargo],
  { encoding: "utf8" },
)
  .split(/\r?\n/)[0]
  .trim();
if (!cargoPath) throw new Error("Cargo executable was not found");

const childEnv = { ...process.env };
childEnv.BUGLOGIN_ENV = "local";
childEnv.BUGLOGIN_POSTGRES_URL = databaseUrl;
childEnv.BUGLOGIN_POSTGRES_TLS_ROOT_CERT =
  localEnv.get("BUGLOGIN_POSTGRES_TLS_ROOT_CERT") ??
  "C:\\ProgramData\\BugLogin\\pg-tls\\root-ca.crt";
if (runtimeCatalogJson && runtimeTrustKeysJson) {
  childEnv.BUGLOGIN_RUNTIME_CATALOG_JSON = runtimeCatalogJson;
  childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON = runtimeTrustKeysJson;
  delete childEnv.BUGLOGIN_RUNTIME_CATALOG_PATH;
  delete childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_PATH;
} else if (runtimeCatalogPath && runtimeTrustKeysPath) {
  childEnv.BUGLOGIN_RUNTIME_CATALOG_PATH = runtimeCatalogPath;
  childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_PATH = runtimeTrustKeysPath;
  delete childEnv.BUGLOGIN_RUNTIME_CATALOG_JSON;
  delete childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON;
} else {
  delete childEnv.BUGLOGIN_RUNTIME_CATALOG_PATH;
  delete childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_PATH;
  delete childEnv.BUGLOGIN_RUNTIME_CATALOG_JSON;
  delete childEnv.BUGLOGIN_RUNTIME_TRUST_KEYS_JSON;
}
childEnv.BUGLOGIN_API_BIND =
  localEnv.get("BUGLOGIN_API_BIND") ?? "127.0.0.1:12345";
childEnv.BUGLOGIN_MCP_ENABLED =
  process.env.BUGLOGIN_MCP_ENABLED ??
  localEnv.get("BUGLOGIN_MCP_ENABLED") ??
  "true";
if (!/^(true|false)$/i.test(childEnv.BUGLOGIN_MCP_ENABLED)) {
  throw new Error("BUGLOGIN_MCP_ENABLED must be true or false");
}
for (const name of [
  "BUGLOGIN_S3_ENDPOINT",
  "BUGLOGIN_S3_BUCKET",
  "BUGLOGIN_S3_REGION",
  "BUGLOGIN_S3_PREFIX",
  "BUGLOGIN_S3_ACCESS_KEY",
  "BUGLOGIN_S3_SECRET_KEY",
  "BUGLOGIN_S3_SESSION_TOKEN",
  "BUGLOGIN_S3_PATH_STYLE",
  "BUGLOGIN_TENANT_ID",
  "BUGLOGIN_REDIS_URL",
  "BUGLOGIN_REDIS_NAMESPACE",
  "BUGLOGIN_REDIS_CA_CERT",
]) {
  if (localEnv.has(name)) childEnv[name] = localEnv.get(name);
}
childEnv.CARGO_TARGET_DIR = cargoTargetDir;
delete childEnv.BUGLOGIN_DATA_ROOT;

const child = spawn(
  cargoPath,
  ["run", "--locked", "-p", "buglogin-api", "--bin", "buglogin-api"],
  { cwd: repositoryRoot, env: childEnv, stdio: "inherit" },
);

child.on("error", (error) => {
  console.error(`Could not start local BugLogin API: ${error.message}`);
  process.exitCode = 1;
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
});

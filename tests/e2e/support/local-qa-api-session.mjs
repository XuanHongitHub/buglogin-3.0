#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  access,
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "../../../scripts/build-target.mjs";

const root = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const workspaceManifestPath = join(root, "Cargo.toml");
const targetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const apiBinary = join(
  targetDir,
  "debug",
  process.platform === "win32" ? "buglogin-api.exe" : "buglogin-api",
);
const bootstrapBinary = join(
  targetDir,
  "debug",
  process.platform === "win32"
    ? "buglogin-dev-bootstrap.exe"
    : "buglogin-dev-bootstrap",
);
const runtimeCatalogPath = join(
  root,
  "apps",
  "desktop",
  "src-tauri",
  "runtime-catalog.json",
);
const runtimeTrustKeysPath = join(
  root,
  "apps",
  "desktop",
  "src-tauri",
  "runtime-trust-keys.json",
);
const runId = randomBytes(6).toString("hex");
const portValue = process.env.BUGLOGIN_QA_API_PORT ?? "12345";
const port = Number(portValue);
const baseUrl = `http://127.0.0.1:${port}`;
const ownerPassword = `Qa!${randomBytes(40).toString("base64url")}`;
const qaEmail = `qa-${runId}@buglogin.test`;
const qaUsername = `qa-${runId}`;
const qaAccountId = `acct_qa_${runId}`;
const qaPassword = `Qa!${randomBytes(40).toString("base64url")}`;
const ownerDeviceId = `local-qa-owner-${runId}`;
const qaDeviceId = `local-qa-member-${runId}`;
let dataRoot;
let api;
let stagedApiBinary;
let stagedBootstrapBinary;
let shuttingDown;
let apiExit;
let runtimeTrustKeysJson;
const validationSessions = [];

function safeCode(value) {
  return typeof value === "string" && /^[a-z0-9_]{1,80}$/.test(value)
    ? value
    : "unknown_problem";
}

function isolatedEnv(extra = {}) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (
      key.startsWith("BUGLOGIN_") ||
      key.startsWith("AWS_") ||
      key === "DATABASE_URL"
    ) {
      delete env[key];
    }
  }
  return {
    ...env,
    BUGLOGIN_ENV: "local",
    BUGLOGIN_DATA_ROOT: dataRoot,
    BUGLOGIN_RUNTIME_CATALOG_PATH: runtimeCatalogPath,
    BUGLOGIN_RUNTIME_TRUST_KEYS_JSON: runtimeTrustKeysJson,
    CARGO_TARGET_DIR: targetDir,
    ...extra,
  };
}

function protectPath(path, directory) {
  if (process.platform === "win32") {
    const identity = execFileSync("whoami.exe", [], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    if (!identity || /[\r\n]/.test(identity)) {
      throw new Error(
        "Could not determine the current Windows account for QA file access.",
      );
    }
    const grant = directory ? `${identity}:(OI)(CI)F` : `${identity}:(F)`;
    execFileSync("icacls.exe", [path, "/inheritance:r", "/grant:r", grant], {
      stdio: "ignore",
    });
    return;
  }
  return chmod(path, directory ? 0o700 : 0o600);
}

function checkTargetDir() {
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    throw new Error("BUGLOGIN_QA_API_PORT must be an integer from 1 to 65535.");
  }
}

async function assertBinaries() {
  for (const binary of [apiBinary, bootstrapBinary]) {
    try {
      await access(binary);
    } catch {
      throw new Error(
        `Missing debug binary ${binary}. Build them with: cargo build -p buglogin-api --bin buglogin-api --bin buglogin-dev-bootstrap`,
      );
    }
  }
}

async function latestRustInputMtimeMs(directory) {
  let latest = 0;
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      latest = Math.max(latest, await latestRustInputMtimeMs(path));
    } else if (
      entry.isFile() &&
      (entry.name === "Cargo.toml" ||
        entry.name === "build.rs" ||
        /\.(?:rs|yaml|yml)$/i.test(entry.name))
    ) {
      latest = Math.max(latest, (await stat(path)).mtimeMs);
    }
  }
  return latest;
}

async function stageBinaries() {
  const workspaceManifest = await readFile(workspaceManifestPath, "utf8");
  const workspacePackage = workspaceManifest.match(
    /\[workspace\.package\]([\s\S]*?)(?=\n\[|$)/,
  )?.[1];
  const sourceVersion = workspacePackage?.match(
    /^version\s*=\s*"([^"]+)"/m,
  )?.[1];
  if (!sourceVersion) {
    throw new Error("Could not read the workspace package version.");
  }
  const [manifestStat, lockStat, crateInputsMtimeMs, controlInputsMtimeMs] =
    await Promise.all([
      stat(workspaceManifestPath),
      stat(join(root, "Cargo.lock")),
      latestRustInputMtimeMs(join(root, "crates")),
      latestRustInputMtimeMs(join(root, "apps", "control")),
    ]);
  const [apiStat, bootstrapStat] = await Promise.all([
    stat(apiBinary),
    stat(bootstrapBinary),
  ]);
  const apiTime = apiStat.mtime.toISOString();
  const bootstrapTime = bootstrapStat.mtime.toISOString();
  const manifestTime = manifestStat.mtime.toISOString();
  const latestSourceTime = Math.max(
    manifestStat.mtimeMs,
    lockStat.mtimeMs,
    crateInputsMtimeMs,
    controlInputsMtimeMs,
  );
  console.log(
    `sourceVersion=${sourceVersion}; sourceManifestMtime=${manifestTime}; latestRustInputMtime=${new Date(latestSourceTime).toISOString()}; apiBinaryMtime=${apiTime}; bootstrapBinaryMtime=${bootstrapTime}`,
  );
  if (
    apiStat.mtimeMs < latestSourceTime ||
    bootstrapStat.mtimeMs < latestSourceTime
  ) {
    throw new Error(
      `Debug API/bootstrap binaries predate source version ${sourceVersion}; rebuild both with: cargo build -p buglogin-api --bin buglogin-api --bin buglogin-dev-bootstrap`,
    );
  }

  const binaryDirectory = join(dataRoot, "bin");
  await mkdir(binaryDirectory, { recursive: true });
  await protectPath(binaryDirectory, true);
  stagedApiBinary = join(
    binaryDirectory,
    process.platform === "win32" ? "buglogin-api.exe" : "buglogin-api",
  );
  stagedBootstrapBinary = join(
    binaryDirectory,
    process.platform === "win32"
      ? "buglogin-dev-bootstrap.exe"
      : "buglogin-dev-bootstrap",
  );
  await Promise.all([
    copyFile(apiBinary, stagedApiBinary),
    copyFile(bootstrapBinary, stagedBootstrapBinary),
  ]);
  if (process.platform !== "win32") {
    await Promise.all([
      chmod(stagedApiBinary, 0o700),
      chmod(stagedBootstrapBinary, 0o700),
    ]);
  }
  const [apiAfter, bootstrapAfter] = await Promise.all([
    stat(apiBinary),
    stat(bootstrapBinary),
  ]);
  if (
    apiAfter.mtimeMs !== apiStat.mtimeMs ||
    apiAfter.size !== apiStat.size ||
    bootstrapAfter.mtimeMs !== bootstrapStat.mtimeMs ||
    bootstrapAfter.size !== bootstrapStat.size
  ) {
    throw new Error(
      "Cargo replaced a debug binary while it was being staged; rerun the QA launcher after the build finishes.",
    );
  }
}

async function loadRuntimeTrustKeys() {
  try {
    await access(runtimeCatalogPath);
    await access(runtimeTrustKeysPath);
  } catch {
    throw new Error(
      "Checked-in signed runtime catalog or trust-key file is missing.",
    );
  }
  const trustKeys = JSON.parse(await readFile(runtimeTrustKeysPath, "utf8"));
  if (
    !trustKeys ||
    typeof trustKeys !== "object" ||
    Array.isArray(trustKeys) ||
    Object.keys(trustKeys).length === 0 ||
    Object.values(trustKeys).some(
      (key) => typeof key !== "string" || !/^[0-9a-f]{64}$/i.test(key),
    )
  ) {
    throw new Error("Checked-in runtime trust-key map is invalid.");
  }
  runtimeTrustKeysJson = JSON.stringify(trustKeys);
}

async function allocatePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(port, "127.0.0.1", resolve);
  });
  await new Promise((resolve, reject) =>
    probe.close((error) => (error ? reject(error) : resolve())),
  );
}

async function runBootstrap() {
  const child = spawn(stagedBootstrapBinary, [], {
    cwd: root,
    env: isolatedEnv(),
    stdio: ["pipe", "pipe", "ignore"],
  });
  let stdout = "";
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    stdout += chunk;
  });
  child.stdin.end(`${ownerPassword}\n`);

  const exitCode = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error("Local QA owner bootstrap timed out."));
    }, 60_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(
        new Error(
          `Could not start local QA bootstrap (${error.code ?? "spawn_error"}).`,
        ),
      );
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });
  if (exitCode !== 0) {
    throw new Error(`Local QA owner bootstrap failed (exit ${exitCode}).`);
  }
  const match = stdout.match(
    /^Local owner ready\. accountId=(acct_[^ ]+) organizationId=(org_[^ ]+) workspaceId=(ws_[^\r\n ]+)/m,
  );
  if (!match)
    throw new Error("Local QA bootstrap returned no owner scope IDs.");
  return {
    ownerAccountId: match[1],
    organizationId: match[2],
    workspaceId: match[3],
  };
}

async function waitForApi() {
  let lastStatus = "not_started";
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (api.exitCode !== null) {
      throw new Error(
        `Local QA API exited before readiness (exit ${api.exitCode}).`,
      );
    }
    try {
      const response = await fetch(`${baseUrl}/v1/readyz`, {
        headers: { "x-correlation-id": `local-qa-${runId}-ready` },
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) {
        if (response.headers.get("x-buglogin-persistence-mode") !== "sqlite") {
          throw new Error(
            "Local QA API is not using isolated SQLite persistence.",
          );
        }
        if (
          response.headers.get("x-buglogin-runtime-catalog") !== "available"
        ) {
          throw new Error(
            "Local QA API did not verify the checked-in runtime catalog.",
          );
        }
        return;
      }
      lastStatus = `http_${response.status}`;
    } catch (error) {
      if (
        error instanceof Error &&
        (error.message.includes("isolated SQLite") ||
          error.message.includes("runtime catalog"))
      )
        throw error;
      lastStatus = "connection_pending";
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Local QA API did not become ready (${lastStatus}).`);
}

async function startApi() {
  api = spawn(stagedApiBinary, [], {
    cwd: root,
    env: isolatedEnv({ BUGLOGIN_API_BIND: `127.0.0.1:${port}` }),
    stdio: "ignore",
  });
  apiExit = new Promise((resolve) => api.once("exit", resolve));
  await waitForApi();
}

async function verifyRuntimeCatalog() {
  const response = await fetch(`${baseUrl}/v1/runtime/catalog`, {
    headers: { "x-correlation-id": `local-qa-${runId}-runtime-catalog` },
    signal: AbortSignal.timeout(3_000),
  });
  if (!response.ok) {
    throw new Error(
      `Local QA runtime catalog returned HTTP ${response.status}.`,
    );
  }
  const catalog = await response.json();
  const windowsWayfern = Array.isArray(catalog.releases)
    ? catalog.releases.filter(
        (release) =>
          release.runtime === "wayfern" &&
          release.platform === "windows" &&
          release.architecture === "x64",
      )
    : [];
  if (
    catalog.schemaVersion !== 2 ||
    windowsWayfern.length === 0 ||
    windowsWayfern.some(
      (release) =>
        typeof release.signature !== "string" ||
        typeof release.sha256 !== "string" ||
        !release.keyId,
    )
  ) {
    throw new Error(
      "Verified runtime catalog has no signed Windows Wayfern release.",
    );
  }
  return windowsWayfern.length;
}

async function request(
  path,
  { method = "GET", token, body, idempotencyKey } = {},
) {
  const headers = { "x-correlation-id": `local-qa-${runId}` };
  if (token) headers.authorization = `Bearer ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10_000),
  });
  let value;
  try {
    value = await response.json();
  } catch {
    value = undefined;
  }
  if (!response.ok) {
    throw new Error(
      `${method} ${path.split("?")[0]} failed (HTTP ${response.status}, code=${safeCode(value?.code)}).`,
    );
  }
  return value;
}

async function login(email, password, deviceId) {
  const result = await request("/v1/auth/desktop/login", {
    method: "POST",
    body: { identifier: email, password, deviceId },
  });
  if (typeof result.accessToken === "string" && result.accessToken.length > 0) {
    validationSessions.push({ deviceId, token: result.accessToken });
  }
  if (result.tokenType !== "Bearer" || typeof result.accessToken !== "string") {
    throw new Error("Local QA login returned an invalid session response.");
  }
  return result.accessToken;
}

async function logoutValidationSessions() {
  let firstError;
  const remaining = [];
  for (const session of validationSessions) {
    try {
      await request("/v1/auth/logout", {
        method: "POST",
        token: session.token,
        idempotencyKey: `local-qa-${runId}-${session.deviceId}-validation-logout`,
        body: {},
      });
    } catch (error) {
      firstError ??= error;
      remaining.push(session);
    }
  }
  validationSessions.splice(0, validationSessions.length, ...remaining);
  if (firstError) throw firstError;
}

async function createQaMember(scopes) {
  const { organizationId, workspaceId } = scopes;
  const accountResult = await request(
    `/v1/organizations/${encodeURIComponent(organizationId)}/accounts`,
    {
      method: "POST",
      token: scopes.ownerToken,
      idempotencyKey: `local-qa-${runId}-account-create`,
      body: {
        organizationId,
        accountId: qaAccountId,
        username: qaUsername,
        email: qaEmail,
        initialPassword: qaPassword,
      },
    },
  );
  if (accountResult.loginEnabled !== true) {
    throw new Error("Local QA member was provisioned without login access.");
  }

  const route = `/v1/organizations/${encodeURIComponent(organizationId)}/workspaces/${encodeURIComponent(workspaceId)}/members`;
  await request(route, {
    method: "POST",
    token: scopes.ownerToken,
    idempotencyKey: `local-qa-${runId}-workspace-assign`,
    body: {
      organizationId,
      workspaceId,
      organizationMemberId: accountResult.organizationMember.id,
      organizationMemberRevision: accountResult.organizationMember.revision,
      role: "member",
    },
  });
  return accountResult;
}

async function writeCredentials(scopes, qaAccount) {
  const path = join(dataRoot, "qa-credentials.json");
  const value = {
    schemaVersion: 1,
    apiBaseUrl: baseUrl,
    owner: {
      label: "local QA organization owner",
      email: "local-owner@buglogin.test",
      password: ownerPassword,
      accountId: scopes.ownerAccountId,
    },
    member: {
      label: "local QA workspace member",
      email: qaEmail,
      password: qaPassword,
      accountId: qaAccount.account.id,
    },
    organizationId: scopes.organizationId,
    workspaceId: scopes.workspaceId,
    deviceLabels: {
      ownerClient: ownerDeviceId,
      memberClient: qaDeviceId,
    },
  };
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
    mode: 0o600,
  });
  await protectPath(path, false);
  return path;
}

function safeToRemoveTempRoot(path) {
  const resolved = resolve(path);
  return (
    dirname(resolved) === resolve(tmpdir()) &&
    resolved.split(/[\\/]/).at(-1)?.startsWith("buglogin-local-qa-api-") ===
      true
  );
}

async function stopApi() {
  if (!api || api.exitCode !== null || api.signalCode !== null) return;
  api.kill("SIGTERM");
  const waitForExit = async (milliseconds) =>
    Promise.race([
      apiExit.then(() => true),
      new Promise((resolve) => setTimeout(() => resolve(false), milliseconds)),
    ]);
  let stopped = await waitForExit(8_000);
  if (!stopped) {
    if (process.platform === "win32" && api.pid) {
      try {
        execFileSync("taskkill.exe", ["/PID", String(api.pid), "/T", "/F"], {
          stdio: "ignore",
        });
      } catch {
        api.kill("SIGKILL");
      }
    } else {
      api.kill("SIGKILL");
    }
    stopped = await waitForExit(5_000);
  }
  if (!stopped)
    throw new Error("Local QA API did not stop; disposable data was retained.");
}

async function cleanup() {
  if (!dataRoot || !safeToRemoveTempRoot(dataRoot)) return;
  await rm(dataRoot, { recursive: true, force: true });
}

async function shutdown(exitCode = 0) {
  if (shuttingDown) return shuttingDown;
  shuttingDown = (async () => {
    await stopApi();
    await cleanup();
    process.exitCode = exitCode;
  })();
  return shuttingDown;
}

async function main() {
  checkTargetDir();
  await assertBinaries();
  await loadRuntimeTrustKeys();
  await allocatePort();
  dataRoot = await mkdtemp(join(tmpdir(), "buglogin-local-qa-api-"));
  await protectPath(dataRoot, true);
  await stageBinaries();

  const owner = await runBootstrap();
  await startApi();
  const runtimeReleaseCount = await verifyRuntimeCatalog();
  const ownerToken = await login(
    "local-owner@buglogin.test",
    ownerPassword,
    ownerDeviceId,
  );
  const member = await createQaMember({
    ...owner,
    ownerToken,
  });
  await login(qaEmail, qaPassword, qaDeviceId);
  const credentialsFile = await writeCredentials(owner, member);
  await logoutValidationSessions();

  console.log(
    "Local QA API ready (isolated SQLite + local chunk storage + verified signed runtime catalog). No external API or production data is used.",
  );
  console.log(`apiBaseUrl=${baseUrl}`);
  console.log(`verifiedWindowsWayfernReleaseCount=${runtimeReleaseCount}`);
  console.log(`credentialsFile=${credentialsFile}`);
  console.log(`organizationId=${owner.organizationId}`);
  console.log(`workspaceId=${owner.workspaceId}`);
  console.log(
    `ownerLabel=local QA organization owner; ownerEmail=local-owner@buglogin.test; ownerAccountId=${owner.ownerAccountId}`,
  );
  console.log(
    `memberLabel=local QA workspace member; memberEmail=${qaEmail}; memberAccountId=${member.account.id}`,
  );
  console.log(`deviceLabels=owner:${ownerDeviceId},member:${qaDeviceId}`);
  console.log(
    "Set BUGLOGIN_API_BASE_URL to this loopback URL for each debug client. Stop with Ctrl-C; the API and disposable data will be removed.",
  );

  await new Promise((resolve) => {
    process.once("SIGINT", resolve);
    process.once("SIGTERM", resolve);
  });
  await shutdown(0);
}

try {
  await main();
} catch (error) {
  try {
    await logoutValidationSessions();
  } catch {
    console.error("Local QA validation session cleanup failed.");
  }
  await shutdown(1);
  console.error(
    error instanceof Error ? error.message : "Local QA API setup failed.",
  );
}

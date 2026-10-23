import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { resolveBuildTargetDir } from "../../../scripts/build-target.mjs";

const execFileAsync = promisify(execFile);
const root = resolve(import.meta.dirname, "../../..");
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
const dataRoot = await mkdtemp(join(tmpdir(), "buglogin-api-process-e2e-"));
const password = "Synthetic-E2E-Password-2026!";
let api;
let apiOutput = "";

function isolatedEnv(extra = {}) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (key.startsWith("BUGLOGIN_")) delete env[key];
  }
  return {
    ...env,
    BUGLOGIN_ENV: "local",
    BUGLOGIN_DATA_ROOT: dataRoot,
    CARGO_TARGET_DIR: targetDir,
    ...extra,
  };
}

async function freeLoopbackPort() {
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const { port } = server.address();
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  );
  return port;
}

async function runBootstrap() {
  const child = spawn(bootstrapBinary, [], {
    cwd: root,
    env: isolatedEnv(),
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr += chunk;
  });
  child.stdin.end(`${password}\n`);
  const code = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error("synthetic owner bootstrap timed out"));
    }, 60_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("exit", (exitCode) => {
      clearTimeout(timer);
      resolve(exitCode);
    });
  });
  assert.equal(code, 0, `synthetic owner bootstrap failed: ${stderr}`);
  assert.match(
    stdout,
    /^Local owner ready\. accountId=acct_[^ ]+ organizationId=org_[^ ]+ workspaceId=ws_[^\r\n]+/m,
  );
}

async function waitForApi(baseUrl, child) {
  let lastError = "API did not respond";
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (child.exitCode !== null)
      throw new Error(`API exited ${child.exitCode}: ${apiOutput}`);
    try {
      const response = await fetch(`${baseUrl}/v1/readyz`, {
        headers: { "x-correlation-id": "local-api-process-smoke-ready" },
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) return response;
      lastError = `readiness returned HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`API did not become ready (${lastError}): ${apiOutput}`);
}

async function requestJsonResponse(
  url,
  { method = "GET", token, body, idempotencyKey, actorId } = {},
) {
  const headers = { "x-correlation-id": "local-api-process-smoke" };
  if (token) headers.authorization = `Bearer ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
  if (actorId) headers["x-buglogin-actor-id"] = actorId;
  let response;
  try {
    response = await fetch(url, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(5_000),
    });
  } catch (error) {
    throw new Error(
      [
        `${method} ${url} network failure: ${error instanceof Error ? error.message : String(error)}`,
        `API output: ${apiOutput}`,
      ].join("; "),
    );
  }
  const text = await response.text();
  let value;
  try {
    value = text ? JSON.parse(text) : undefined;
  } catch {
    value = text;
  }
  return { response, value };
}

async function requestJson(url, options = {}) {
  const { response, value } = await requestJsonResponse(url, options);
  assert.ok(
    response.ok,
    `${options.method ?? "GET"} ${url} returned HTTP ${response.status}: ${JSON.stringify(value)}`,
  );
  return { response, value };
}

async function startApi(port) {
  apiOutput = "";
  const child = spawn(apiBinary, [], {
    cwd: root,
    env: isolatedEnv({ BUGLOGIN_API_BIND: `127.0.0.1:${port}` }),
    stdio: ["ignore", "pipe", "pipe"],
  });
  api = child;
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    apiOutput += chunk;
  });
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    apiOutput += chunk;
  });
  const ready = await waitForApi(`http://127.0.0.1:${port}`, child);
  assert.equal(ready.headers.get("x-buglogin-persistence-mode"), "sqlite");
}

async function stopApi(child) {
  const hasStopped = () => child.exitCode !== null || child.signalCode !== null;
  if (hasStopped()) return;
  child.kill();
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  assert.ok(hasStopped(), "API child process did not stop cleanly");
}

try {
  if (process.env.BUGLOGIN_E2E_SKIP_BUILD !== "1") {
    await execFileAsync(
      process.platform === "win32" ? "cargo.exe" : "cargo",
      [
        "build",
        "--locked",
        "-q",
        "-p",
        "buglogin-api",
        "--bin",
        "buglogin-api",
        "--bin",
        "buglogin-dev-bootstrap",
      ],
      {
        cwd: root,
        env: isolatedEnv(),
        timeout: 300_000,
        maxBuffer: 8 * 1024 * 1024,
      },
    );
  }

  await runBootstrap();
  let port = await freeLoopbackPort();
  let baseUrl = `http://127.0.0.1:${port}`;
  await startApi(port);

  const launcherCheck = await execFileAsync(
    process.execPath,
    [join(root, "scripts", "tauri-dev-local-api.mjs"), "--check"],
    {
      cwd: root,
      env: isolatedEnv({
        BUGLOGIN_DEV_API_BASE_URL: baseUrl,
        BUGLOGIN_DEV_CARGO_TARGET_DIR: targetDir,
      }),
      timeout: 5_000,
      maxBuffer: 1024 * 1024,
    },
  );
  assert.ok(launcherCheck.stdout.includes(baseUrl));

  const invalidLogin = await requestJsonResponse(
    `${baseUrl}/v1/auth/desktop/login`,
    {
      method: "POST",
      body: {
        identifier: "local-owner@buglogin.test",
        password: "incorrect-e2e-password",
        deviceId: "e2e-local-api-invalid-device",
      },
    },
  );
  assert.equal(invalidLogin.response.status, 401);
  assert.equal(invalidLogin.value.code, "invalid_credentials");
  assert.ok(!("accessToken" in invalidLogin.value));
  assert.ok(!("refreshToken" in invalidLogin.value));

  const { value: login } = await requestJson(
    `${baseUrl}/v1/auth/desktop/login`,
    {
      method: "POST",
      body: {
        identifier: "local-owner@buglogin.test",
        password,
        deviceId: "e2e-local-api-device",
      },
    },
  );
  assert.equal(login.tokenType, "Bearer");
  assert.ok(login.accessToken);

  const { value: account } = await requestJson(`${baseUrl}/v1/account`, {
    token: login.accessToken,
  });
  assert.match(account.accountId, /^acct_/);
  const { value: organizations } = await requestJson(
    `${baseUrl}/v1/organizations`,
    { token: login.accessToken },
  );
  assert.equal(organizations.length, 1);
  assert.equal(organizations[0].accountId, account.accountId);
  const { value: workspaces } = await requestJson(
    `${baseUrl}/v1/organizations/${encodeURIComponent(organizations[0].id)}/workspaces`,
    { token: login.accessToken },
  );
  assert.equal(workspaces.length, 1);
  assert.equal(workspaces[0].name, "Main Workspace");

  const workspaceId = workspaces[0].id;

  const grantPageResponse = await requestJson(
    `${baseUrl}/v1/workspaces/${encodeURIComponent(workspaceId)}/grants/page?limit=1`,
    { token: login.accessToken },
  );
  assert.equal(grantPageResponse.response.status, 200);
  assert.deepEqual(Object.keys(grantPageResponse.value).sort(), [
    "items",
    "nextCursor",
  ]);
  assert.ok(Array.isArray(grantPageResponse.value.items));
  assert.ok(grantPageResponse.value.items.length <= 1);
  const proxyId = "proxy_e2e_local_01";
  const proxyRoute = `/v1/workspaces/${encodeURIComponent(workspaceId)}/proxies`;
  let proxyPath = `${baseUrl}${proxyRoute}/${proxyId}`;
  let proxyCollectionPath = `${baseUrl}${proxyRoute}`;
  const proxyFields = {
    id: proxyId,
    name: "Local E2E proxy",
    protocol: "http",
    host: "127.0.0.1",
    port: 3128,
    bypassRules: [],
    createdAt: "2026-10-19T00:00:00Z",
    updatedAt: "2026-10-19T00:00:00Z",
  };
  const { value: createdProxy } = await requestJson(proxyCollectionPath, {
    method: "POST",
    token: login.accessToken,
    idempotencyKey: "local-api-process-proxy-create-001",
    body: { workspaceId, proxy: proxyFields },
  });
  assert.equal(createdProxy.deduplicated, false);
  assert.equal(createdProxy.record.revision, 1);
  assert.equal(createdProxy.record.proxy.id, proxyId);

  const { value: proxyPage } = await requestJson(proxyCollectionPath, {
    token: login.accessToken,
  });
  assert.equal(proxyPage.items.length, 1);
  assert.equal(proxyPage.items[0].proxy.name, "Local E2E proxy");

  const { value: updatedProxy } = await requestJson(proxyPath, {
    method: "PATCH",
    token: login.accessToken,
    idempotencyKey: "local-api-process-proxy-update-001",
    body: {
      workspaceId,
      proxyId,
      expectedRevision: 1,
      proxy: {
        ...proxyFields,
        name: "Local E2E proxy updated",
        updatedAt: "2026-10-19T00:01:00Z",
      },
    },
  });
  assert.equal(updatedProxy.revision, 2);

  const profileId = "prf_e2e_local_01";
  let profilePath = `${baseUrl}/v1/profiles/${profileId}`;
  let profileCollectionPath = `${baseUrl}/v1/profiles`;
  const { value: createdProfile } = await requestJson(profileCollectionPath, {
    method: "POST",
    token: login.accessToken,
    idempotencyKey: "local-api-process-profile-create-001",
    body: {
      id: profileId,
      name: "Local E2E profile",
      runtime: "wayfern",
      workspaceId,
    },
  });
  assert.equal(createdProfile.value.id, profileId);
  assert.equal(createdProfile.revision, 1);
  const { value: profileBinding } = await requestJson(
    `${baseUrl}/v1/profiles/${profileId}/binding`,
    { token: login.accessToken },
  );
  assert.equal(profileBinding.profileId, profileId);
  assert.equal(profileBinding.workspaceId, workspaceId);
  const manifestPath = `${baseUrl}/v1/workspaces/${workspaceId}/resources/${profileBinding.resourceId}/manifest`;
  const { value: initialManifest } = await requestJson(manifestPath, {
    token: login.accessToken,
    actorId: account.accountId,
  });
  assert.equal(initialManifest.generation, 1);
  assert.deepEqual(initialManifest.objects, []);

  const { value: listedProfiles } = await requestJson(profileCollectionPath, {
    token: login.accessToken,
  });
  assert.ok(
    listedProfiles.items.some((item) => item.value.id === profileId),
    "created profile should appear in the local profile collection",
  );

  await stopApi(api);
  port = await freeLoopbackPort();
  baseUrl = `http://127.0.0.1:${port}`;
  proxyPath = `${baseUrl}${proxyRoute}/${proxyId}`;
  proxyCollectionPath = `${baseUrl}${proxyRoute}`;
  profilePath = `${baseUrl}/v1/profiles/${profileId}`;
  profileCollectionPath = `${baseUrl}/v1/profiles`;
  await startApi(port);

  const { value: secondLogin } = await requestJson(
    `${baseUrl}/v1/auth/desktop/login`,
    {
      method: "POST",
      body: {
        identifier: "local-owner@buglogin.test",
        password,
        deviceId: "e2e-local-api-device-after-restart",
      },
    },
  );
  const { value: restoredOrganizations } = await requestJson(
    `${baseUrl}/v1/organizations`,
    { token: secondLogin.accessToken },
  );
  assert.equal(restoredOrganizations.length, 1);
  const { value: restoredWorkspaces } = await requestJson(
    `${baseUrl}/v1/organizations/${encodeURIComponent(restoredOrganizations[0].id)}/workspaces`,
    { token: secondLogin.accessToken },
  );
  assert.equal(restoredWorkspaces.length, 1);
  assert.equal(restoredWorkspaces[0].id, workspaceId);

  const { value: restoredBinding } = await requestJson(
    `${baseUrl}/v1/profiles/${profileId}/binding`,
    { token: secondLogin.accessToken },
  );
  assert.equal(restoredBinding.resourceId, profileBinding.resourceId);
  const { value: restoredManifest } = await requestJson(
    `${baseUrl}/v1/workspaces/${workspaceId}/resources/${restoredBinding.resourceId}/manifest`,
    { token: secondLogin.accessToken, actorId: account.accountId },
  );
  assert.equal(restoredManifest.generation, 1);
  assert.deepEqual(restoredManifest.objects, []);

  const { value: persistedProxy } = await requestJson(proxyPath, {
    token: secondLogin.accessToken,
  });
  assert.equal(persistedProxy.revision, 2);
  assert.equal(persistedProxy.proxy.name, "Local E2E proxy updated");

  const { value: persistedProfile } = await requestJson(profilePath, {
    token: secondLogin.accessToken,
  });
  assert.equal(persistedProfile.value.id, profileId);
  assert.equal(persistedProfile.revision, 1);
  const { value: persistedProfilePage } = await requestJson(
    profileCollectionPath,
    { token: secondLogin.accessToken },
  );
  assert.ok(
    persistedProfilePage.items.some((item) => item.value.id === profileId),
    "profile should remain listed after API process restart",
  );
  const { value: deletedProfile } = await requestJson(profilePath, {
    method: "DELETE",
    token: secondLogin.accessToken,
    idempotencyKey: "local-api-process-profile-delete-001",
    body: {
      revision: persistedProfile.revision,
      confirmation: "QUARANTINE_PROFILE_DATA",
    },
  });
  assert.equal(deletedProfile.id, profileId);
  assert.equal(deletedProfile.deleted, true);
  assert.equal(deletedProfile.recoverable, true);
  const { value: profilesAfterDelete } = await requestJson(
    profileCollectionPath,
    { token: secondLogin.accessToken },
  );
  assert.ok(
    !profilesAfterDelete.items.some((item) => item.value.id === profileId),
    "deleted profile should no longer appear in the local profile collection",
  );

  const { value: deletedProxy } = await requestJson(proxyPath, {
    method: "DELETE",
    token: secondLogin.accessToken,
    idempotencyKey: "local-api-process-proxy-delete-001",
    body: { workspaceId, proxyId, expectedRevision: 2 },
  });
  assert.equal(deletedProxy.revision, 2);
  const { value: emptyProxyPage } = await requestJson(proxyCollectionPath, {
    token: secondLogin.accessToken,
  });
  assert.equal(emptyProxyPage.items.length, 0);

  const refreshToken = secondLogin.refreshToken;
  assert.equal(typeof refreshToken, "string");
  const refreshed = await requestJson(`${baseUrl}/v1/auth/device/refresh`, {
    method: "POST",
    body: {
      deviceId: "e2e-local-api-device-after-restart",
      refreshToken,
    },
  });
  assert.equal(refreshed.response.headers.get("cache-control"), "no-store");
  assert.ok(refreshed.value.accessToken);
  assert.ok(refreshed.value.refreshToken);
  assert.notEqual(refreshed.value.accessToken, secondLogin.accessToken);
  assert.notEqual(refreshed.value.refreshToken, refreshToken);

  const replayedRefresh = await requestJsonResponse(
    `${baseUrl}/v1/auth/device/refresh`,
    {
      method: "POST",
      body: {
        deviceId: "e2e-local-api-device-after-restart",
        refreshToken,
      },
    },
  );
  assert.equal(replayedRefresh.response.status, 401);
  assert.equal(replayedRefresh.value.code, "session_revoked");

  const revokedAccess = await requestJsonResponse(`${baseUrl}/v1/account`, {
    token: refreshed.value.accessToken,
  });
  assert.equal(revokedAccess.response.status, 401);
  assert.equal(revokedAccess.value.code, "session_revoked");

  console.log(
    "PASS local API process E2E: credential rejection, login, CRUD, API restart persistence, refresh rotation/replay revocation",
  );
} finally {
  if (api && api.exitCode === null) await stopApi(api);
  await rm(dataRoot, { recursive: true, force: true });
}

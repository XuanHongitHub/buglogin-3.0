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
const dataRoot = await mkdtemp(
  join(tmpdir(), "buglogin-workspace-switch-e2e-"),
);
const password = "Synthetic-Workspace-E2E-2026!";
let api;
let apiOutput = "";

function isolatedEnv(extra = {}) {
  const env = { ...process.env };
  for (const key of Object.keys(env))
    if (key.startsWith("BUGLOGIN_")) delete env[key];
  return {
    ...env,
    BUGLOGIN_ENV: "local",
    BUGLOGIN_DATA_ROOT: dataRoot,
    CARGO_TARGET_DIR: targetDir,
    ...extra,
  };
}

async function freePort() {
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

async function bootstrap() {
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
      reject(new Error("owner bootstrap timed out"));
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
  assert.equal(code, 0, `owner bootstrap failed: ${stderr}`);
  const match = stdout.match(
    /^Local owner ready\. accountId=(acct_[^ ]+) organizationId=(org_[^ ]+) workspaceId=(ws_[^\r\n]+)/m,
  );
  assert.ok(match, `bootstrap did not return tenant IDs: ${stdout}`);
  return {
    accountId: match[1],
    organizationId: match[2],
    initialWorkspaceId: match[3],
  };
}

async function startApi(port) {
  apiOutput = "";
  api = spawn(apiBinary, [], {
    cwd: root,
    env: isolatedEnv({ BUGLOGIN_API_BIND: `127.0.0.1:${port}` }),
    stdio: ["ignore", "pipe", "pipe"],
  });
  api.stdout.setEncoding("utf8").on("data", (chunk) => {
    apiOutput += chunk;
  });
  api.stderr.setEncoding("utf8").on("data", (chunk) => {
    apiOutput += chunk;
  });
  for (let i = 0; i < 80; i += 1) {
    if (api.exitCode !== null)
      throw new Error(`API exited ${api.exitCode}: ${apiOutput}`);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/v1/readyz`, {
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) {
        assert.equal(
          response.headers.get("x-buglogin-persistence-mode"),
          "sqlite",
        );
        return;
      }
    } catch {
      /* retry while local API starts */
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`API did not become ready: ${apiOutput}`);
}

async function stopApi() {
  if (!api || api.exitCode !== null || api.signalCode !== null) return;
  api.kill();
  await Promise.race([
    new Promise((resolve) => api.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  assert.ok(
    api.exitCode !== null || api.signalCode !== null,
    "API did not stop",
  );
}

async function request(
  baseUrl,
  path,
  { method = "GET", token, body, idempotencyKey } = {},
) {
  const headers = { "x-correlation-id": "local-api-workspace-switch-e2e" };
  if (token) headers.authorization = `Bearer ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(5_000),
  });
  const text = await response.text();
  let value;
  try {
    value = text ? JSON.parse(text) : undefined;
  } catch {
    value = text;
  }
  assert.ok(
    response.ok,
    `${method} ${path} returned ${response.status}: ${JSON.stringify(value)}`,
  );
  return value;
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

  const tenant = await bootstrap();
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  await startApi(port);

  const login = await request(baseUrl, "/v1/auth/desktop/login", {
    method: "POST",
    body: {
      identifier: "local-owner@buglogin.test",
      password,
      deviceId: "workspace-switch-e2e-device",
    },
  });
  assert.equal(login.tokenType, "Bearer");
  assert.ok(login.accessToken);
  const auth = { token: login.accessToken };
  const account = await request(baseUrl, "/v1/account", auth);
  assert.equal(account.accountId, tenant.accountId);

  const organizations = await request(baseUrl, "/v1/organizations", auth);
  assert.equal(organizations.length, 1);
  assert.equal(organizations[0].id, tenant.organizationId);
  const organizationId = organizations[0].id;
  const workspaceListPath = `/v1/organizations/${encodeURIComponent(organizationId)}/workspaces`;
  let workspaces = await request(baseUrl, workspaceListPath, auth);
  assert.equal(workspaces.length, 1);
  assert.equal(workspaces[0].id, tenant.initialWorkspaceId);

  const secondWorkspaceId = "ws_e2e_switch_target";
  const secondWorkspace = await request(baseUrl, workspaceListPath, {
    ...auth,
    method: "POST",
    idempotencyKey: "workspace-switch-create-002",
    body: {
      id: secondWorkspaceId,
      organizationId,
      name: "Workspace Switch Target",
      policy: {
        maxProfiles: 100,
        maxMembers: 10,
        maxStorageBytes: 1_000_000,
        offlineEntitled: false,
      },
      createdAt: "2026-10-19T00:00:00Z",
    },
  });
  assert.equal(
    secondWorkspace.id,
    secondWorkspaceId,
    JSON.stringify(secondWorkspace),
  );
  workspaces = await request(baseUrl, workspaceListPath, auth);
  assert.deepEqual(
    workspaces.map((item) => item.id).sort(),
    [tenant.initialWorkspaceId, secondWorkspaceId].sort(),
  );

  // The API has no mutable active-workspace session field. A client switches by
  // selecting an authorized workspace ID and scoping each request to that ID.
  // Verify the resulting tenancy boundary on both sides of the switch.
  const workspaceAProxyPath = `/v1/workspaces/${encodeURIComponent(tenant.initialWorkspaceId)}/proxies`;
  const workspaceBProxyPath = `/v1/workspaces/${encodeURIComponent(secondWorkspaceId)}/proxies`;
  const proxy = {
    id: "proxy_switch_e2e",
    name: "Workspace B Proxy",
    protocol: "http",
    host: "127.0.0.1",
    port: 8080,
    bypassRules: [],
    createdAt: "2026-10-19T00:00:00Z",
    updatedAt: "2026-10-19T00:00:00Z",
  };
  const createdProxy = await request(baseUrl, workspaceBProxyPath, {
    ...auth,
    method: "POST",
    idempotencyKey: "workspace-switch-proxy-create-001",
    body: { workspaceId: secondWorkspaceId, proxy },
  });
  assert.equal(createdProxy.record.proxy.id, proxy.id);
  assert.equal(
    (await request(baseUrl, workspaceAProxyPath, auth)).items.length,
    0,
  );
  assert.equal(
    (await request(baseUrl, workspaceBProxyPath, auth)).items.length,
    1,
  );

  const profileId = "prf_switch_e2e";
  const createdProfile = await request(baseUrl, "/v1/profiles", {
    ...auth,
    method: "POST",
    idempotencyKey: "workspace-switch-profile-create-001",
    body: {
      id: profileId,
      name: "Workspace B Profile",
      runtime: "wayfern",
      workspaceId: secondWorkspaceId,
      proxyId: proxy.id,
    },
  });
  assert.equal(createdProfile.value.id, profileId);
  assert.equal(createdProfile.value.proxyId, proxy.id);
  const profileBinding = await request(
    baseUrl,
    `/v1/profiles/${encodeURIComponent(profileId)}/binding`,
    auth,
  );
  assert.equal(profileBinding.workspaceId, secondWorkspaceId);
  const listProfiles = async (workspaceId) =>
    request(
      baseUrl,
      `/v1/profiles?workspaceId=${encodeURIComponent(workspaceId)}`,
      auth,
    );
  assert.deepEqual((await listProfiles(tenant.initialWorkspaceId)).items, []);
  assert.ok(
    (await listProfiles(secondWorkspaceId)).items.some(
      (item) => item.value.id === profileId,
    ),
  );

  // Switch back to A, then return to B and assert each workspace's rows stay isolated.
  assert.equal(
    (await request(baseUrl, workspaceAProxyPath, auth)).items.length,
    0,
  );
  assert.equal((await listProfiles(tenant.initialWorkspaceId)).items.length, 0);
  assert.equal(
    (await request(baseUrl, workspaceBProxyPath, auth)).items[0].proxy.id,
    proxy.id,
  );
  assert.equal(
    (await listProfiles(secondWorkspaceId)).items[0].value.id,
    profileId,
  );

  console.log(
    "PASS local API workspace switch process E2E: direct login, organization/workspace list, create second workspace, switch scope A↔B, profile/proxy CRUD visibility isolation",
  );
} finally {
  await stopApi();
  await rm(dataRoot, { recursive: true, force: true });
}

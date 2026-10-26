import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";
import { mkdtemp } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { resolveBuildTargetDir } from "../../../scripts/build-target.mjs";

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
const dataRoot = await mkdtemp(join(tmpdir(), "buglogin-encrypted-sync-e2e-"));
const password = "Synthetic-Sync-E2E-Password-2026!";
const runId = randomBytes(6).toString("hex");
const s3Environment = (() => {
  if (process.env.BUGLOGIN_SYNC_E2E_S3 !== "1") return {};
  const required = [
    "BUGLOGIN_S3_ENDPOINT",
    "BUGLOGIN_S3_BUCKET",
    "BUGLOGIN_S3_ACCESS_KEY",
    "BUGLOGIN_S3_SECRET_KEY",
  ];
  for (const name of required) {
    if (!process.env[name]?.trim()) throw new Error(`${name} is required for S3 sync E2E`);
  }
  if (new URL(process.env.BUGLOGIN_S3_ENDPOINT).protocol !== "https:") {
    throw new Error("S3 sync E2E requires an HTTPS endpoint");
  }
  return {
    BUGLOGIN_S3_ENDPOINT: process.env.BUGLOGIN_S3_ENDPOINT,
    BUGLOGIN_S3_BUCKET: process.env.BUGLOGIN_S3_BUCKET,
    BUGLOGIN_S3_REGION: process.env.BUGLOGIN_S3_REGION ?? "us-east-1",
    BUGLOGIN_S3_PREFIX: process.env.BUGLOGIN_S3_PREFIX ?? "buglogin3/local-dev",
    BUGLOGIN_S3_ACCESS_KEY: process.env.BUGLOGIN_S3_ACCESS_KEY,
    BUGLOGIN_S3_SECRET_KEY: process.env.BUGLOGIN_S3_SECRET_KEY,
    BUGLOGIN_S3_PATH_STYLE: process.env.BUGLOGIN_S3_PATH_STYLE ?? "false",
    BUGLOGIN_TENANT_ID: `buglogin3-qa-${runId}`,
    ...(process.env.BUGLOGIN_S3_SESSION_TOKEN
      ? { BUGLOGIN_S3_SESSION_TOKEN: process.env.BUGLOGIN_S3_SESSION_TOKEN }
      : {}),
  };
})();
let api;
let apiOutput = "";
let baseUrl;

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
    ...s3Environment,
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
  child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
  child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
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
  assert.match(stdout, /^Local owner ready\./m);
}

async function waitForApi(child) {
  let lastError = "API did not respond";
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (child.exitCode !== null)
      throw new Error(`API exited ${child.exitCode}: ${apiOutput}`);
    try {
      const response = await fetch(`${baseUrl}/v1/readyz`, {
        headers: { "x-correlation-id": `encrypted-sync-${runId}-ready` },
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) {
        assert.equal(
          response.headers.get("x-buglogin-persistence-mode"),
          "sqlite",
        );
        return;
      }
      lastError = `readiness returned HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`API did not become ready (${lastError}): ${apiOutput}`);
}

async function startApi(port) {
  apiOutput = "";
  baseUrl = `http://127.0.0.1:${port}`;
  const child = spawn(apiBinary, [], {
    cwd: root,
    env: isolatedEnv({ BUGLOGIN_API_BIND: `127.0.0.1:${port}` }),
    stdio: ["ignore", "pipe", "pipe"],
  });
  api = child;
  child.stdout.setEncoding("utf8").on("data", (chunk) => (apiOutput += chunk));
  child.stderr.setEncoding("utf8").on("data", (chunk) => (apiOutput += chunk));
  await waitForApi(child);
}

async function stopApi(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill();
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  assert.ok(
    child.exitCode !== null || child.signalCode !== null,
    "isolated API process did not stop cleanly",
  );
}

async function request(
  url,
  { method = "GET", token, body, headers = {} } = {},
) {
  const allHeaders = {
    "x-correlation-id": `encrypted-sync-${runId}`,
    ...headers,
  };
  if (token) allHeaders.authorization = `Bearer ${token}`;
  if (body !== undefined && !(body instanceof Uint8Array))
    allHeaders["content-type"] = "application/json";
  const response = await fetch(url, {
    method,
    headers: allHeaders,
    body:
      body === undefined
        ? undefined
        : body instanceof Uint8Array
          ? body
          : JSON.stringify(body),
    signal: AbortSignal.timeout(10_000),
  });
  const bytes = Buffer.from(await response.arrayBuffer());
  let value;
  try {
    value = bytes.length ? JSON.parse(bytes.toString("utf8")) : undefined;
  } catch {
    value = bytes;
  }
  assert.ok(
    response.ok,
    `${method} ${url} returned HTTP ${response.status}: ${
      Buffer.isBuffer(value) ? "<binary>" : JSON.stringify(value)
    }`,
  );
  return { response, value, bytes };
}

function checksum(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function encryptAesGcm(key, plaintext, aad) {
  const nonce = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  cipher.setAAD(Buffer.from(aad));
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  const tag = cipher.getAuthTag();
  return {
    jsonChunk: {
      version: 1,
      nonce: [...nonce],
      ciphertext: [...ciphertext],
      tag: [...tag],
    },
    wireBytes: Buffer.concat([Buffer.from([1]), nonce, tag, ciphertext]),
  };
}

function decryptAesGcm(key, { nonce, ciphertext, tag }, aad) {
  const decipher = createDecipheriv("aes-256-gcm", key, Buffer.from(nonce));
  decipher.setAAD(Buffer.from(aad));
  decipher.setAuthTag(Buffer.from(tag));
  return Buffer.concat([
    decipher.update(Buffer.from(ciphertext)),
    decipher.final(),
  ]);
}

function decryptWireChunk(key, wireBytes, aad) {
  assert.equal(wireBytes[0], 1, "encrypted chunk version should be supported");
  const nonce = wireBytes.subarray(1, 13);
  const tag = wireBytes.subarray(13, 29);
  const ciphertext = wireBytes.subarray(29);
  const decipher = createDecipheriv("aes-256-gcm", key, nonce);
  decipher.setAAD(Buffer.from(aad));
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]);
}

function objectRef(key, bytes) {
  return { key, size: bytes.length, checksum: checksum(bytes) };
}

try {
  await runBootstrap();
  await startApi(await freeLoopbackPort());

  const login = async (deviceId) => {
    const { value } = await request(`${baseUrl}/v1/auth/desktop/login`, {
      method: "POST",
      body: {
        identifier: "local-owner@buglogin.test",
        password,
        deviceId,
      },
    });
    assert.equal(value.tokenType, "Bearer");
    assert.ok(value.accessToken);
    return value.accessToken;
  };

  const deviceOneToken = await login(`sync-e2e-device-one-${runId}`);
  const { value: account } = await request(`${baseUrl}/v1/account`, {
    token: deviceOneToken,
  });
  const { value: organizations } = await request(
    `${baseUrl}/v1/organizations`,
    {
      token: deviceOneToken,
    },
  );
  assert.equal(organizations.length, 1);
  const { value: workspaces } = await request(
    `${baseUrl}/v1/organizations/${encodeURIComponent(organizations[0].id)}/workspaces`,
    { token: deviceOneToken },
  );
  assert.equal(workspaces.length, 1);
  const workspaceId = workspaces[0].id;
  const profileId = `prf_e2e_sync_${runId}`;
  const proxyId = `pxy_e2e_sync_${runId}`;
  const proxyFields = {
    id: proxyId,
    name: `Encrypted sync proxy ${runId}`,
    protocol: "http",
    host: "127.0.0.1",
    port: 3128,
    hasCredential: true,
    bypassRules: [],
    createdAt: "2026-10-19T00:00:00Z",
    updatedAt: "2026-10-19T00:00:00Z",
  };
  const proxyRoute = `/v1/workspaces/${encodeURIComponent(workspaceId)}/proxies`;
  const { value: createdProxy } = await request(`${baseUrl}${proxyRoute}`, {
    method: "POST",
    token: deviceOneToken,
    headers: { "idempotency-key": `sync-e2e-proxy-create-${runId}` },
    body: { workspaceId, proxy: proxyFields },
  });
  assert.equal(createdProxy.record.proxy.id, proxyId);
  assert.equal(createdProxy.record.revision, 1);
  assert.equal(createdProxy.record.proxy.hasCredential, true);
  assert.ok(createdProxy.record.proxy.credentialVersion);
  assert.doesNotMatch(JSON.stringify(createdProxy), /sync-e2e-proxy-secret/);

  const created = await request(`${baseUrl}/v1/profiles`, {
    method: "POST",
    token: deviceOneToken,
    headers: { "idempotency-key": `sync-e2e-profile-create-${runId}` },
    body: {
      id: profileId,
      name: `Encrypted sync E2E ${runId}`,
      runtime: "wayfern",
      workspaceId,
      proxyId,
    },
  });
  assert.equal(created.value.value.id, profileId);
  assert.equal(created.value.value.proxyId, proxyId);
  const { value: binding } = await request(
    `${baseUrl}/v1/profiles/${profileId}/binding`,
    { token: deviceOneToken },
  );
  const { value: profile } = await request(
    `${baseUrl}/v1/profiles/${profileId}`,
    {
      token: deviceOneToken,
    },
  );
  const resourceId = binding.resourceId;

  const ownerId = `sync-e2e-device-one-${runId}`;
  const scopePath = `/v1/workspaces/${encodeURIComponent(workspaceId)}/resources/${encodeURIComponent(resourceId)}`;
  const { value: lease } = await request(`${baseUrl}${scopePath}/lease`, {
    method: "POST",
    token: deviceOneToken,
    headers: { "idempotency-key": `sync-e2e-lease-${runId}` },
    body: { ownerId, ttlSeconds: 120 },
  });

  const resourceKey = randomBytes(32);
  const recoveryKey = randomBytes(32);
  const envelopeChunk = encryptAesGcm(
    recoveryKey,
    resourceKey,
    `buglogin/resource-key/v1/${workspaceId}/${resourceId}`,
  );
  const envelopeBytes = Buffer.from(
    JSON.stringify({
      schema_version: 1,
      workspace_id: workspaceId,
      resource_id: resourceId,
      encrypted_key: envelopeChunk.jsonChunk,
    }),
  );
  const profileChunk = encryptAesGcm(
    resourceKey,
    Buffer.from(`synthetic encrypted profile payload ${runId}`),
    `buglogin-e2e-profile-payload-${runId}`,
  );
  const envelopeObject = objectRef(
    ".buglogin/resource-key-envelope.json",
    envelopeBytes,
  );
  const proxyCredential = encryptAesGcm(
    resourceKey,
    Buffer.from(
      JSON.stringify({
        username: "sync-e2e-proxy-user",
        password: "sync-e2e-proxy-secret",
      }),
    ),
    `buglogin/proxy-credential/v1/${workspaceId}/${resourceId}/${proxyId}/${createdProxy.record.revision}`,
  );
  const proxyCredentialEnvelopeBytes = Buffer.from(
    JSON.stringify({
      schemaVersion: 1,
      workspaceId,
      resourceId,
      proxyId,
      proxyRevision: createdProxy.record.revision,
      encryptedSecret: proxyCredential.jsonChunk,
    }),
  );
  const proxyCredentialObject = objectRef(
    `.buglogin/proxy-dependencies/${proxyId}/${createdProxy.record.revision}/credential-envelope.json`,
    proxyCredentialEnvelopeBytes,
  );
  const payloadObject = objectRef(
    `Default/Preferences-${runId}`,
    profileChunk.wireBytes,
  );
  const writeHeaders = {
    "idempotency-key": `sync-e2e-object-${runId}-envelope`,
    "x-buglogin-owner-id": lease.ownerId,
    "x-buglogin-owner-token": lease.ownerToken,
    "x-buglogin-fencing-token": String(lease.fencingToken),
    "x-buglogin-generation": "2",
    "x-content-sha256": envelopeObject.checksum,
  };
  await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(envelopeObject.key)}`,
    {
      method: "PUT",
      token: deviceOneToken,
      headers: writeHeaders,
      body: envelopeBytes,
    },
  );
  await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(payloadObject.key)}`,
    {
      method: "PUT",
      token: deviceOneToken,
      headers: {
        ...writeHeaders,
        "idempotency-key": `sync-e2e-object-${runId}-payload`,
        "x-content-sha256": payloadObject.checksum,
      },
      body: profileChunk.wireBytes,
    },
  );
  await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(proxyCredentialObject.key)}`,
    {
      method: "PUT",
      token: deviceOneToken,
      headers: {
        ...writeHeaders,
        "idempotency-key": `sync-e2e-object-${runId}-proxy-credential`,
        "x-content-sha256": proxyCredentialObject.checksum,
      },
      body: proxyCredentialEnvelopeBytes,
    },
  );

  const manifest = {
    schemaVersion: 2,
    workspaceId,
    resourceId,
    fencingToken: lease.fencingToken,
    generation: 2,
    parentGeneration: 1,
    rootChecksum: checksum(Buffer.from(`synthetic profile root ${runId}`)),
    objects: [envelopeObject, payloadObject, proxyCredentialObject],
    tombstones: [],
    dependencies: [
      {
        kind: "proxy",
        proxyId,
        revision: createdProxy.record.revision,
        credentialVersion: createdProxy.record.proxy.credentialVersion,
        credentialEnvelopeObject: proxyCredentialObject,
      },
    ],
    payloadEncryption: {
      schemaVersion: 1,
      algorithm: "aes-256-gcm-chunk-v1",
      resourceKeyEnvelope: envelopeObject,
    },
  };
  const { value: committed } = await request(
    `${baseUrl}${scopePath}/manifest`,
    {
      method: "PUT",
      token: deviceOneToken,
      headers: {
        "idempotency-key": `sync-e2e-manifest-commit-${runId}`,
        "x-buglogin-owner-id": lease.ownerId,
        "x-buglogin-owner-token": lease.ownerToken,
        "x-buglogin-fencing-token": String(lease.fencingToken),
        "x-buglogin-profile-revision": String(profile.revision),
      },
      body: manifest,
    },
  );
  assert.equal(committed.generation, 2);
  assert.equal(committed.objects.length, 3);
  assert.equal(committed.dependencies[0].proxyId, proxyId);

  const deviceTwoToken = await login(`sync-e2e-device-two-${runId}`);
  const { value: deviceTwoProxies } = await request(`${baseUrl}${proxyRoute}`, {
    token: deviceTwoToken,
  });
  const deviceTwoProxy = deviceTwoProxies.items.find(
    (item) => item.proxy.id === proxyId,
  );
  assert.ok(
    deviceTwoProxy,
    "second device must see the assigned workspace proxy",
  );
  assert.equal(deviceTwoProxy.revision, createdProxy.record.revision);
  assert.equal(deviceTwoProxy.proxy.host, proxyFields.host);
  assert.equal(deviceTwoProxy.proxy.hasCredential, true);
  assert.equal(
    deviceTwoProxy.proxy.credentialVersion,
    createdProxy.record.proxy.credentialVersion,
  );

  const { value: deviceTwoProfiles } = await request(
    `${baseUrl}/v1/profiles?workspaceId=${encodeURIComponent(workspaceId)}`,
    { token: deviceTwoToken },
  );
  const deviceTwoProfile = deviceTwoProfiles.items.find(
    (item) => item.value.id === profileId,
  );
  assert.ok(
    deviceTwoProfile,
    "second device must see the synced profile record",
  );
  assert.equal(deviceTwoProfile.value.proxyId, proxyId);

  const { value: remoteManifest } = await request(
    `${baseUrl}${scopePath}/manifest`,
    {
      token: deviceTwoToken,
      headers: { "x-buglogin-actor-id": account.accountId },
    },
  );
  assert.equal(remoteManifest.generation, 2);
  assert.equal(remoteManifest.dependencies[0].proxyId, proxyId);
  assert.equal(
    remoteManifest.dependencies[0].credentialVersion,
    createdProxy.record.proxy.credentialVersion,
  );
  assert.equal(
    remoteManifest.dependencies[0].credentialEnvelopeObject.key,
    proxyCredentialObject.key,
  );
  assert.deepEqual(
    remoteManifest.objects.map(({ key, checksum: hash }) => [key, hash]),
    manifest.objects.map(({ key, checksum: hash }) => [key, hash]),
  );
  const { bytes: downloadedProxyCredential } = await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(proxyCredentialObject.key)}`,
    {
      token: deviceTwoToken,
      headers: {
        "x-buglogin-actor-id": account.accountId,
        "x-buglogin-generation": "2",
        "x-content-sha256": proxyCredentialObject.checksum,
      },
    },
  );
  assert.equal(
    checksum(downloadedProxyCredential),
    proxyCredentialObject.checksum,
  );
  const downloadedProxyEnvelope = JSON.parse(
    downloadedProxyCredential.toString("utf8"),
  );
  assert.equal(downloadedProxyEnvelope.proxyRevision, deviceTwoProxy.revision);
  const decryptedProxyCredentials = decryptAesGcm(
    resourceKey,
    downloadedProxyEnvelope.encryptedSecret,
    `buglogin/proxy-credential/v1/${workspaceId}/${resourceId}/${proxyId}/${deviceTwoProxy.revision}`,
  );
  assert.deepEqual(JSON.parse(decryptedProxyCredentials.toString("utf8")), {
    username: "sync-e2e-proxy-user",
    password: "sync-e2e-proxy-secret",
  });
  const { bytes: downloadedEnvelope } = await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(envelopeObject.key)}`,
    {
      token: deviceTwoToken,
      headers: {
        "x-buglogin-actor-id": account.accountId,
        "x-buglogin-generation": "2",
        "x-content-sha256": envelopeObject.checksum,
      },
    },
  );
  const { bytes: downloadedPayload } = await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(payloadObject.key)}`,
    {
      token: deviceTwoToken,
      headers: {
        "x-buglogin-actor-id": account.accountId,
        "x-buglogin-generation": "2",
        "x-content-sha256": payloadObject.checksum,
      },
    },
  );
  assert.deepEqual(downloadedEnvelope, envelopeBytes);
  assert.deepEqual(downloadedPayload, profileChunk.wireBytes);
  assert.equal(checksum(downloadedPayload), payloadObject.checksum);

  const downloadedEnvelopeJson = JSON.parse(
    downloadedEnvelope.toString("utf8"),
  );
  const recoveredResourceKey = decryptAesGcm(
    recoveryKey,
    downloadedEnvelopeJson.encrypted_key,
    `buglogin/resource-key/v1/${workspaceId}/${resourceId}`,
  );
  assert.deepEqual(recoveredResourceKey, resourceKey);
  const expectedPlaintext = Buffer.from(
    `synthetic encrypted profile payload ${runId}`,
  );
  const decryptedPayload = decryptWireChunk(
    recoveredResourceKey,
    downloadedPayload,
    `buglogin-e2e-profile-payload-${runId}`,
  );
  assert.deepEqual(decryptedPayload, expectedPlaintext);
  assert.throws(
    () =>
      decryptWireChunk(
        randomBytes(32),
        downloadedPayload,
        `buglogin-e2e-profile-payload-${runId}`,
      ),
    /Unsupported state|authentication tag|bad decrypt/i,
    "a wrong resource key must fail AES-GCM authentication",
  );
  const tamperedPayload = Buffer.from(downloadedPayload);
  tamperedPayload[13] ^= 1;
  assert.throws(
    () =>
      decryptWireChunk(
        recoveredResourceKey,
        tamperedPayload,
        `buglogin-e2e-profile-payload-${runId}`,
      ),
    /Unsupported state|authentication tag|bad decrypt/i,
    "a modified authentication tag must fail AES-GCM authentication",
  );

  await stopApi(api);
  api = undefined;
  await startApi(await freeLoopbackPort());
  const afterRestartToken = await login(
    `sync-e2e-device-after-restart-${runId}`,
  );
  const { value: persistedManifest } = await request(
    `${baseUrl}${scopePath}/manifest`,
    {
      token: afterRestartToken,
      headers: { "x-buglogin-actor-id": account.accountId },
    },
  );
  assert.equal(persistedManifest.generation, 2);
  const { bytes: persistedPayload } = await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(payloadObject.key)}`,
    {
      token: afterRestartToken,
      headers: {
        "x-buglogin-actor-id": account.accountId,
        "x-buglogin-generation": "2",
        "x-content-sha256": payloadObject.checksum,
      },
    },
  );
  assert.deepEqual(persistedPayload, profileChunk.wireBytes);
  const { bytes: persistedProxyCredential } = await request(
    `${baseUrl}${scopePath}/blobs?key=${encodeURIComponent(proxyCredentialObject.key)}`,
    {
      token: afterRestartToken,
      headers: {
        "x-buglogin-actor-id": account.accountId,
        "x-buglogin-generation": "2",
        "x-content-sha256": proxyCredentialObject.checksum,
      },
    },
  );
  assert.deepEqual(persistedProxyCredential, downloadedProxyCredential);

  console.log(
    `PASS ${Object.keys(s3Environment).length ? "S3" : "local"} API encrypted sync process E2E: cross-device profile/proxy/credential sync, AES-GCM payload and proxy-credential decrypt, wrong-key/tag rejection, manifest commit/read, checksum, API restart persistence`,
  );
} finally {
  await stopApi(api);
  // The data root is created exclusively by mkdtemp for this run.
  const { rm } = await import("node:fs/promises");
  await rm(dataRoot, { recursive: true, force: true });
}

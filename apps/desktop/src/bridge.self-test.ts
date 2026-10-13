import assert from "node:assert/strict";
import { createTauriDesktopApi } from "./bridge.ts";

const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
const invoke = async <Result>(
  command: string,
  args?: Record<string, unknown>,
): Promise<Result> => {
  calls.push({ command, args });
  if (command === "desktop_auth_status")
    return { state: "unauthenticated" } as Result;
  if (command === "desktop_auth_device_start")
    return {
      state: "authorizing",
      userCode: "ABCD-1234",
      verificationUri: "https://app.example/device",
      expiresAt: 100,
      interval: 5,
    } as Result;
  return {
    commandId: "profile.create",
    correlationId: "cor_test",
    data: {
      id: "profile_1",
      name: "Primary",
      runtime: "wayfern",
      state: "ready",
    },
  } as Result;
};

const api = createTauriDesktopApi(invoke);
assert.deepEqual(await api.authStatus(), { state: "unauthenticated" });
const auth = await api.startDeviceAuthorization();
assert.equal(auth.state, "authorizing");
assert.equal(calls[0]?.command, "desktop_auth_status");
assert.equal(calls[1]?.command, "desktop_auth_device_start");
assert.equal(JSON.stringify(auth).includes("deviceCode"), false);
assert.equal(JSON.stringify(auth).includes("accessToken"), false);
await api.createProfile({ name: "Primary", runtime: "wayfern" });

assert.equal(calls[2]?.command, "profile_create");
assert.deepEqual(calls[2]?.args?.payload, {
  name: "Primary",
  runtime: "wayfern",
});
assert.match(String(calls[2]?.args?.correlationId), /^cor_/);
assert.match(String(calls[2]?.args?.idempotencyKey), /^idem_/);

await api.cloneProfile({
  sourceProfileId: "profile_1",
  targetProfileId: "profile_2",
  name: "Clone",
  expectedRevision: 7,
});
await api.shareProfile({
  profileId: "profile_1",
  memberId: "member_1",
  permission: "operate",
  expectedRevision: 8,
});
await api.revokeProfileShare({
  profileId: "profile_1",
  memberId: "member_1",
  permission: "operate",
  expectedRevision: 9,
});
assert.equal(calls[3]?.command, "profile_clone");
assert.deepEqual(calls[3]?.args?.payload, {
  sourceProfileId: "profile_1",
  targetProfileId: "profile_2",
  name: "Clone",
  expectedRevision: 7,
});
assert.match(String(calls[3]?.args?.idempotencyKey), /^idem_/);
assert.equal(calls[4]?.command, "profile_share");
assert.equal(calls[5]?.command, "profile_share_revoke");
assert.deepEqual(calls[5]?.args?.payload, {
  profileId: "profile_1",
  memberId: "member_1",
  permission: "operate",
  expectedRevision: 9,
});

await api.listProfiles(true);
await api.archiveProfile("profile_1", 7);
assert.equal(calls[6]?.command, "profile_list");
assert.deepEqual(calls[6]?.args?.payload, { includeArchived: true });
assert.equal(calls[7]?.command, "profile_archive");
assert.deepEqual(calls[7]?.args?.payload, {
  profileId: "profile_1",
  revision: 7,
});
assert.match(String(calls[7]?.args?.idempotencyKey), /^idem_/);

await api.listProxies("ws_1");
await api.getProxy("ws_1", "proxy_1");
await api.createProxy({
  workspaceId: "ws_1",
  proxy: {
    id: "proxy_1",
    protocol: "http",
    host: "proxy.example",
    port: 8080,
    bypassRules: [],
    createdAt: "2026-08-21T00:00:00Z",
    updatedAt: "2026-08-21T00:00:00Z",
  },
});
await api.updateProxy({
  workspaceId: "ws_1",
  proxyId: "proxy_1",
  expectedRevision: 1,
  proxy: {
    id: "proxy_1",
    protocol: "http",
    host: "proxy.example",
    port: 8081,
    bypassRules: [],
    createdAt: "2026-08-21T00:00:00Z",
    updatedAt: "2026-08-21T00:00:00Z",
  },
});
await api.deleteProxy({
  workspaceId: "ws_1",
  proxyId: "proxy_1",
  expectedRevision: 2,
});
assert.equal(calls[8]?.command, "proxy_list");
assert.deepEqual(calls[8]?.args?.payload, { workspaceId: "ws_1" });
assert.equal(calls[9]?.command, "proxy_get");
assert.deepEqual(calls[9]?.args?.payload, {
  workspaceId: "ws_1",
  proxyId: "proxy_1",
});
assert.equal(calls[10]?.command, "proxy_create");
assert.match(String(calls[10]?.args?.idempotencyKey), /^idem_/);
assert.equal(calls[11]?.command, "proxy_update");
assert.match(String(calls[11]?.args?.idempotencyKey), /^idem_/);
assert.equal(calls[12]?.command, "proxy_delete");
assert.deepEqual(calls[12]?.args?.payload, {
  workspaceId: "ws_1",
  proxyId: "proxy_1",
  expectedRevision: 2,
});
assert.match(String(calls[12]?.args?.idempotencyKey), /^idem_/);

await api.listGrants("ws_1", "res_1");
await api.createGrant({
  id: "grant_1",
  workspaceId: "ws_1",
  resourceId: "res_1",
  memberId: "mem_1",
  action: "read",
  effect: "allow",
});
await api.revokeGrant("grant_1");
assert.equal(calls[13]?.command, "grant_list");
assert.deepEqual(calls[13]?.args?.payload, {
  workspaceId: "ws_1",
  resourceId: "res_1",
});
assert.equal(calls[14]?.command, "grant_create");
assert.match(String(calls[14]?.args?.idempotencyKey), /^idem_/);
assert.equal(calls[15]?.command, "grant_revoke");
assert.deepEqual(calls[15]?.args?.payload, { grantId: "grant_1" });

await api.inspectMigration("C:/staging/legacy");
assert.equal(calls[16]?.command, "migration_inspect");
assert.deepEqual(calls[16]?.args?.payload, { sourceRoot: "C:/staging/legacy" });
assert.match(String(calls[16]?.args?.correlationId), /^cor_/);
assert.match(String(calls[16]?.args?.idempotencyKey), /^idem_/);

await api.importMigration({
  packageRoot: "C:/package",
  stagingRoot: "C:/staging",
  destinationRoot: "C:/repository",
  runId: "run-1",
  signingKeyNamespace: "migration",
  signingKeyName: "signing",
  migrationKeyNamespace: "migration",
  migrationKeyName: "encryption",
  confirmation: "CUTOVER_AND_PROMOTE",
  dryRunApproved: true,
});
assert.equal(calls[17]?.command, "migration_import");
assert.match(String(calls[17]?.args?.idempotencyKey), /^idem_/);

await api.checkProxy({ workspaceId: "ws_1", proxyId: "proxy_1" });
assert.equal(calls[18]?.command, "proxy_check");
assert.deepEqual(calls[18]?.args?.payload, {
  workspaceId: "ws_1",
  proxyId: "proxy_1",
});

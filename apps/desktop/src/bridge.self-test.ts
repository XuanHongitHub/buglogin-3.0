import assert from "node:assert/strict";
import {
  createTauriDesktopApi,
  DESKTOP_AUTH_STATUS_CHANGED_EVENT,
} from "./bridge.ts";

const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
const invoke = async <Result>(
  command: string,
  args?: Record<string, unknown>,
): Promise<Result> => {
  calls.push({ command, args });
  if (command === "desktop_auth_status")
    return { state: "unauthenticated" } as Result;
  if (command === "desktop_auth_login")
    return { state: "authenticated" } as Result;
  if (command === "desktop_auth_sso_start") return 300 as Result;
  if (command === "desktop_auth_sso_complete")
    return { state: "authenticated" } as Result;
  if (command === "desktop_deep_link_current") return [] as Result;
  if (command === "integrations_status_get")
    return {
      deploymentMode: "production",
      apiEndpoint: "https://api.example.test",
      apiReachable: true,
      apiService: "buglogin-api",
      mcpEndpoint: "https://api.example.test/v1/mcp",
      mcpEnabled: false,
    } as Result;
  if (command === "profile_list")
    return {
      commandId: "profile.list",
      correlationId: "cor_test",
      data: { items: [], nextCursor: null },
    } as Result;
  if (command === "grant_list_page")
    return {
      commandId: "grant.list.page",
      correlationId: "cor_test",
      data: { items: [], nextCursor: null },
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
const auth = await api.login(
  "owner@example.com",
  "correct horse battery staple",
);
assert.equal(auth.state, "authenticated");
assert.equal(calls[0]?.command, "desktop_auth_status");
assert.equal(calls[1]?.command, "desktop_auth_login");
assert.equal(calls[1]?.args?.identifier, "owner@example.com");
assert.equal(calls[1]?.args?.password, "correct horse battery staple");
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
  confirmedDeviceFingerprints: [
    { deviceId: "device_1", fingerprint: "ab".repeat(32) },
  ],
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
assert.deepEqual(calls[4]?.args?.payload, {
  profileId: "profile_1",
  memberId: "member_1",
  permission: "operate",
  expectedRevision: 8,
  confirmedDeviceFingerprints: [
    { deviceId: "device_1", fingerprint: "ab".repeat(32) },
  ],
});
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
assert.deepEqual(calls[6]?.args?.payload, {
  includeArchived: true,
  limit: 100,
});
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
assert.equal(calls[13]?.command, "grant_list_page");
assert.deepEqual(calls[13]?.args?.payload, {
  workspaceId: "ws_1",
  limit: 100,
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

await api.listGroups("ws_1");
await api.getGroup({ workspaceId: "ws_1", groupId: "grp_1" });
await api.createGroup({
  workspaceId: "ws_1",
  groupId: "grp_1",
  name: "North America",
});
await api.updateGroup({
  workspaceId: "ws_1",
  groupId: "grp_1",
  expectedRevision: 2,
  name: "North America team",
});
await api.deleteGroup({
  workspaceId: "ws_1",
  groupId: "grp_1",
  expectedRevision: 3,
});
await api.replaceProfileGroups({
  workspaceId: "ws_1",
  profileId: "profile_1",
  expectedRevision: 4,
  groupIds: ["grp_1"],
});
assert.deepEqual(
  calls.slice(19).map(({ command }) => command),
  [
    "group_list",
    "group_get",
    "group_create",
    "group_update",
    "group_delete",
    "profile_groups_replace",
  ],
);
for (const call of calls.slice(21)) {
  assert.match(String(call.args?.idempotencyKey), /^idem_/);
}
assert.deepEqual(calls[19]?.args?.payload, { workspaceId: "ws_1" });
assert.deepEqual(calls[21]?.args?.payload, {
  workspaceId: "ws_1",
  groupId: "grp_1",
  name: "North America",
});
assert.deepEqual(calls[24]?.args?.payload, {
  workspaceId: "ws_1",
  profileId: "profile_1",
  expectedRevision: 4,
  groupIds: ["grp_1"],
});
await api.listProfileShareRecipientDeviceKeys({
  profileId: "profile_1",
  memberId: "member_1",
});
assert.equal(calls[25]?.command, "profile_share_recipientdevicekeys_list");
assert.deepEqual(calls[25]?.args?.payload, {
  profileId: "profile_1",
  memberId: "member_1",
});
await api.listProfileKeyRotationRecipients("profile_1");
assert.equal(calls[26]?.command, "profile_keyrotation_recipients_list");
assert.deepEqual(calls[26]?.args?.payload, { profileId: "profile_1" });
await api.executeProfileKeyRotation({
  profileId: "profile_1",
  confirmedRecipients: [
    {
      memberId: "member_1",
      deviceId: "device_1",
      fingerprint: "ab".repeat(32),
    },
  ],
});
assert.equal(calls[27]?.command, "profile_keyrotation_execute");
assert.deepEqual(calls[27]?.args?.payload, {
  profileId: "profile_1",
  confirmedRecipients: [
    {
      memberId: "member_1",
      deviceId: "device_1",
      fingerprint: "ab".repeat(32),
    },
  ],
});
assert.match(String(calls[27]?.args?.idempotencyKey), /^idem_/);
await api.stopProfile("profile_1");
assert.equal(calls[28]?.command, "profile_stop");
assert.deepEqual(calls[28]?.args?.payload, { profileId: "profile_1" });
assert.deepEqual(await api.getIntegrationStatus(), {
  deploymentMode: "production",
  apiEndpoint: "https://api.example.test",
  apiReachable: true,
  apiService: "buglogin-api",
  mcpEndpoint: "https://api.example.test/v1/mcp",
  mcpEnabled: false,
});
assert.equal(calls[29]?.command, "integrations_status_get");

const recordCallStart = calls.length;
await api.getOrganizationRecord({ organizationId: "org_1" });
await api.getWorkspaceRecord({ organizationId: "org_1", workspaceId: "ws_1" });
assert.deepEqual(
  calls.slice(recordCallStart).map(({ command, args }) => ({
    command,
    payload: args?.payload,
    correlationId: args?.correlationId,
  })),
  [
    {
      command: "organization_record_get",
      payload: { organizationId: "org_1" },
      correlationId: calls[recordCallStart]?.args?.correlationId,
    },
    {
      command: "workspace_record_get",
      payload: { organizationId: "org_1", workspaceId: "ws_1" },
      correlationId: calls[recordCallStart + 1]?.args?.correlationId,
    },
  ],
);
assert.match(String(calls[recordCallStart]?.args?.correlationId), /^cor_/);
assert.match(String(calls[recordCallStart + 1]?.args?.correlationId), /^cor_/);

const accountCallStart = calls.length;
await api.listAccounts();
await api.createPendingAccount(
  {
    accountId: "acct_created",
    username: "created-user",
    email: "created@example.test",
  },
  "account-create-idempotency-01",
);
await api.updateAccountStatus({
  accountId: "acct_1",
  status: "suspended",
  expectedRevision: 4,
});
assert.deepEqual(
  calls.slice(accountCallStart).map(({ command, args }) => ({
    command,
    payload: args?.payload,
    idempotencyKey: args?.idempotencyKey,
  })),
  [
    { command: "account_list", payload: {}, idempotencyKey: undefined },
    {
      command: "account_create",
      payload: {
        accountId: "acct_created",
        username: "created-user",
        email: "created@example.test",
      },
      idempotencyKey: "account-create-idempotency-01",
    },
    {
      command: "account_status_update",
      payload: {
        accountId: "acct_1",
        status: "suspended",
        expectedRevision: 4,
      },
      idempotencyKey: calls[accountCallStart + 2]?.args?.idempotencyKey,
    },
  ],
);
assert.match(
  String(calls[accountCallStart + 2]?.args?.idempotencyKey),
  /^idem_/,
);

const authEvents: Event[] = [];
Object.defineProperty(globalThis, "window", {
  configurable: true,
  value: { dispatchEvent: (event: Event) => authEvents.push(event) },
});
let revokedInvokeCount = 0;
const revokedApi = createTauriDesktopApi(async () => {
  revokedInvokeCount += 1;
  throw {
    type: "about:blank",
    title: "Session revoked",
    status: 401,
    code: "session_revoked",
    correlationId: "cor_revoked",
    detail: "Session revoked",
    retryable: false,
  };
});
await assert.rejects(revokedApi.launchProfile("profile_1"));
await assert.rejects(revokedApi.stopProfile("profile_1"));
assert.equal(revokedInvokeCount, 2);
assert.deepEqual(
  authEvents.map((event) => event.type),
  [DESKTOP_AUTH_STATUS_CHANGED_EVENT],
);

assert.equal(await api.startBugLabSso(), 300);
assert.deepEqual(
  await api.completeBugLabSso("a".repeat(64), "sso_test_token_123456"),
  { state: "authenticated" },
);
assert.deepEqual(await api.currentDeepLinks(), []);
assert.equal(calls.at(-3)?.command, "desktop_auth_sso_start");
assert.equal(calls.at(-2)?.command, "desktop_auth_sso_complete");
assert.equal(calls.at(-2)?.args?.callbackState, "a".repeat(64));
assert.equal(calls.at(-2)?.args?.token, "sso_test_token_123456");
assert.equal(calls.at(-1)?.command, "desktop_deep_link_current");

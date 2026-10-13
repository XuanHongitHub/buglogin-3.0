import { type GrantCreateInput, type Profile, WebApiError } from "./api.ts";
import { createWebApiClient } from "./http-client.ts";

const controller = new AbortController();
const requests: Request[] = [];
const organization = {
  id: "org:2",
  accountId: "account-1",
  name: "Operations",
  createdAt: "2026-07-22T00:00:00Z",
};
const workspace = {
  id: "workspace:1",
  organizationId: "org:1",
  name: "Production",
  policy: {
    maxProfiles: 12,
    maxMembers: 8,
    maxStorageBytes: 4096,
    offlineEntitled: true,
  },
  createdAt: "2026-07-20T00:00:00Z",
};
const team = {
  id: "team:1",
  workspaceId: "workspace:1",
  name: "Operators",
  memberIds: [],
};
const invitedMember = {
  id: "member:1",
  accountId: "account-2",
  workspaceId: "workspace:1",
  teamId: "team:1",
  role: "member" as const,
  status: "invited" as const,
  inviteExpiresAt: 1_800_000_000,
};
const resourceGrant: GrantCreateInput = {
  id: "grant:1",
  workspaceId: "workspace:1",
  resourceId: "resource:1",
  memberId: "member:1",
  action: "execute",
  effect: "allow",
};
const client = createWebApiClient({
  accessToken: "secret-token",
  baseUrl: "http://127.0.0.1:47821/",
  correlationId: () => "cor_self_test",
  workspaceScopes: [{ organizationId: "org:1", workspaceId: "workspace:1" }],
  fetch: async (input, init) => {
    assert(init?.signal === controller.signal, "AbortSignal was not forwarded");
    const request = new Request(input, init);
    requests.push(request);
    assert(!request.url.includes("secret-token"), "Token leaked into URL");
    assert(
      request.headers.get("authorization") === "Bearer secret-token",
      "Bearer header missing",
    );
    assert(
      request.headers.get("x-correlation-id") === "cor_self_test",
      "Correlation header missing",
    );
    if (request.method === "GET" && request.url.endsWith("/v1/health"))
      return Response.json({ status: "ok", service: "buglogin-api" });
    if (request.method === "GET" && request.url.endsWith("/v1/account"))
      return Response.json({ accountId: "account-1" });
    if (request.method === "GET" && request.url.endsWith("/v1/organizations"))
      return Response.json([
        {
          id: "org:1",
          accountId: "account-1",
          name: "Production",
          createdAt: "2026-07-20T00:00:00Z",
        },
      ]);
    if (
      request.method === "GET" &&
      request.url.endsWith("/v1/organizations/org%3A1/workspaces")
    )
      return Response.json([
        {
          id: "workspace:1",
          organizationId: "org:1",
          name: "Production",
          policy: {
            maxProfiles: 12,
            maxMembers: 8,
            maxStorageBytes: 4096,
            offlineEntitled: true,
          },
          createdAt: "2026-07-20T00:00:00Z",
        },
      ]);
    if (
      request.method === "GET" &&
      request.url.endsWith("/v1/organizations/org%3A1/workspaces/workspace%3A1")
    )
      return Response.json(workspace);
    if (request.method === "POST" && request.url.endsWith("/v1/organizations"))
      return Response.json({
        organization,
        policy: { maxWorkspaces: 4, offlineEntitled: false },
        revision: 1,
      });
    if (
      request.method === "PATCH" &&
      request.url.endsWith("/v1/organizations/org%3A2")
    )
      return Response.json(organization);
    if (
      request.method === "POST" &&
      request.url.endsWith("/v1/organizations/org%3A1/workspaces")
    )
      return Response.json(workspace);
    if (
      request.method === "PATCH" &&
      request.url.endsWith("/v1/organizations/org%3A1/workspaces/workspace%3A1")
    )
      return Response.json(workspace);
    if (
      request.method === "POST" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/teams")
    )
      return Response.json({ team, revision: 1 });
    if (
      request.method === "PUT" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/teams/team%3A1")
    )
      return Response.json({
        team: { ...team, name: "Platform" },
        revision: 2,
      });
    if (
      request.method === "DELETE" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/teams/team%3A1")
    )
      return Response.json({ team, revision: 2 });
    if (
      request.method === "POST" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/members")
    )
      return Response.json(invitedMember);
    if (request.method === "PUT" && request.url.endsWith("/activate"))
      return Response.json({
        ...invitedMember,
        status: "active",
        inviteExpiresAt: null,
      });
    if (request.method === "PUT" && request.url.endsWith("/suspend"))
      return Response.json({
        ...invitedMember,
        status: "suspended",
        inviteExpiresAt: null,
      });
    if (request.method === "PUT" && request.url.endsWith("/role"))
      return Response.json({ ...invitedMember, role: "admin" });
    if (
      request.method === "POST" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/grants")
    )
      return Response.json(resourceGrant);
    if (
      request.method === "DELETE" &&
      request.url.endsWith("/v1/workspaces/workspace%3A1/grants/grant%3A1")
    )
      return Response.json({ grantId: "grant:1", revoked: true });
    return Response.json({}, { status: 404 });
  },
});

const summary = await client.reads.publicSummary(controller.signal);
assert(
  summary.status.label === "Operational",
  "Health response was not mapped",
);
const account = await client.reads.account(controller.signal);
assert(account.accountId === "account-1", "Account response was not parsed");
const workspaces = await client.reads.workspaces(controller.signal);
assert(
  workspaces.length === 1 && workspaces[0].policy.maxMembers === 8,
  "Workspace list was not parsed",
);
const workspaceResult = await client.reads.workspace(
  "workspace:1",
  controller.signal,
);
assert(
  workspaceResult.organizationId === "org:1",
  "Workspace detail was not parsed",
);
assert(requests.length === 4, "Unexpected fetch count");
assert(
  requests.every((request) => request.method === "GET"),
  "Read method mismatch",
);

const mutationOptions = {
  idempotencyKey: "idem-self-test-0001",
  signal: controller.signal,
};
const createdOrganization = await client.writes.createOrganization(
  { organization, policy: { maxWorkspaces: 4, offlineEntitled: false } },
  mutationOptions,
);
assert(
  createdOrganization.organization.id === organization.id &&
    createdOrganization.revision === 1,
  "Organization create response was not parsed",
);
const updatedOrganization = await client.writes.updateOrganization(
  organization.id,
  {
    organization,
    policy: { maxWorkspaces: 4, offlineEntitled: false },
    expectedRevision: 1,
  },
  mutationOptions,
);
assert(
  updatedOrganization.name === "Operations",
  "Organization update response was not parsed",
);
await client.writes.createWorkspace("org:1", workspace, mutationOptions);
await client.writes.updateWorkspace(
  workspace.id,
  { workspace, expectedRevision: 1 },
  mutationOptions,
);
const createdTeam = await client.writes.createTeam(
  workspace.id,
  team,
  mutationOptions,
);
assert(createdTeam.revision === 1, "Team create revision was not parsed");
const updatedTeam = await client.writes.updateTeam(
  workspace.id,
  team.id,
  { name: "Platform", expectedRevision: 1 },
  mutationOptions,
);
assert(
  updatedTeam.team.name === "Platform" && updatedTeam.revision === 2,
  "Team update response was not parsed",
);
const deletedTeam = await client.writes.deleteTeam(
  workspace.id,
  team.id,
  { expectedRevision: 2 },
  mutationOptions,
);
assert(deletedTeam.team.id === team.id, "Team delete response was not parsed");
await client.writes.inviteMember(workspace.id, invitedMember, mutationOptions);
const activatedMember = await client.writes.activateMember(
  workspace.id,
  invitedMember.id,
  mutationOptions,
);
assert(
  activatedMember.status === "active",
  "Member activate response was not parsed",
);
const suspendedMember = await client.writes.suspendMember(
  workspace.id,
  invitedMember.id,
  mutationOptions,
);
assert(
  suspendedMember.status === "suspended",
  "Member suspend response was not parsed",
);
const promotedMember = await client.writes.updateMemberRole(
  workspace.id,
  invitedMember.id,
  { role: "admin" },
  mutationOptions,
);
assert(promotedMember.role === "admin", "Member role response was not parsed");
const createdGrant = await client.writes.createGrant(
  workspace.id,
  resourceGrant,
  mutationOptions,
);
assert(
  createdGrant.id === resourceGrant.id,
  "Grant create response was not parsed",
);
const revokedGrant = await client.writes.revokeGrant(
  workspace.id,
  resourceGrant.id,
  mutationOptions,
);
assert(
  revokedGrant.revoked && revokedGrant.grantId === resourceGrant.id,
  "Grant revoke response was not parsed",
);

let auditRequestUrl = "";
const auditClient = createWebApiClient({
  accessToken: "secret-token",
  baseUrl: "http://127.0.0.1:47821/",
  workspaceScopes: [{ organizationId: "org:1", workspaceId: "workspace:1" }],
  fetch: async (input) => {
    auditRequestUrl = String(input);
    return Response.json({
      items: [
        {
          eventId: "evt_1",
          occurredAt: 1_800_000_000_000,
          actorId: "account-1",
          resource: {
            resourceType: "profile",
            resourceId: "profile-1",
            scope: {
              accountId: null,
              organizationId: null,
              workspaceId: "workspace:1",
              teamId: null,
              memberId: null,
              resourceId: null,
              profileId: null,
            },
          },
          commandId: "profile.create",
          correlationId: "cor_audit",
          outcome: "succeeded",
          metadata: {},
        },
      ],
      nextCursor: null,
    });
  },
});
const auditPage = await auditClient.reads.auditEvents(
  workspace.id,
  "12:evt_0",
  25,
  controller.signal,
);
assert(
  auditPage.items.length === 1 &&
    auditPage.items[0].commandId === "profile.create",
  "Audit page was not parsed",
);
assert(
  auditRequestUrl.endsWith("/audit-events?limit=25&cursor=12%3Aevt_0"),
  "Audit cursor/limit query was not encoded",
);

const mutationRequests = requests.slice(4);
assert(mutationRequests.length === 13, "Unexpected mutation fetch count");
assert(
  mutationRequests.every(
    (request) =>
      request.headers.get("idempotency-key") === mutationOptions.idempotencyKey,
  ),
  "Idempotency header missing",
);
assert(
  mutationRequests.every(
    (request) => request.headers.get("authorization") === "Bearer secret-token",
  ),
  "Mutation bearer header missing",
);
assert(
  mutationRequests.every(
    (request) => request.headers.get("x-correlation-id") === "cor_self_test",
  ),
  "Mutation correlation header missing",
);
assert(
  mutationRequests.map((request) => request.method).join(",") ===
    "POST,PATCH,POST,PATCH,POST,PUT,DELETE,POST,PUT,PUT,PUT,POST,DELETE",
  "Mutation method mismatch",
);
assert(
  await mutationRequests[1]
    .json()
    .then((value) => value.expectedRevision === 1),
  "Organization revision body missing",
);
assert(
  await mutationRequests[5]
    .json()
    .then((value) => value.expectedRevision === 1),
  "Team revision body missing",
);
assert(
  await mutationRequests[6]
    .json()
    .then((value) => value.expectedRevision === 2),
  "Team delete revision body missing",
);
assert(
  mutationRequests[8].headers.get("content-type") === null &&
    (await mutationRequests[8].text()) === "",
  "Activate must not send a body",
);
assert(
  mutationRequests[9].headers.get("content-type") === null &&
    (await mutationRequests[9].text()) === "",
  "Suspend must not send a body",
);
assert(
  await mutationRequests[11]
    .json()
    .then((value) => value.id === resourceGrant.id),
  "Grant create body missing",
);
assert(
  mutationRequests[12].url.endsWith(
    "/v1/workspaces/workspace%3A1/grants/grant%3A1",
  ),
  "Grant revoke path mismatch",
);

const problemClient = createWebApiClient({
  baseUrl: "http://127.0.0.1:47821",
  correlationId: () => "cor_request",
  fetch: async () =>
    Response.json(
      {
        type: "https://buglogin.local/problems/authentication_required",
        title: "Authentication required",
        status: 401,
        code: "authentication_required",
        correlationId: "cor_server",
        retryable: false,
        fields: {},
      },
      { status: 401 },
    ),
});

await expectProblem(
  problemClient.reads.publicSummary(controller.signal),
  "authentication_required",
  "cor_server",
);
await expectProblem(
  problemClient.writes.createOrganization(
    { organization, policy: { maxWorkspaces: 4, offlineEntitled: false } },
    mutationOptions,
  ),
  "authentication_required",
  "cor_server",
);
assert(
  Number(requests.length) === 17,
  "Problem client must not affect primary request log",
);

const invalidClient = createWebApiClient({
  baseUrl: "http://127.0.0.1:47821",
  correlationId: () => "cor_invalid",
  fetch: async () => Response.json({ accountId: 1 }),
});
await expectProblem(
  invalidClient.reads.account(controller.signal),
  "invalid_response",
  "cor_invalid",
);
const invalidOperationClient = createWebApiClient({
  baseUrl: "http://127.0.0.1:47821",
  correlationId: () => "cor_invalid_operation",
  fetch: async () =>
    Response.json({
      operationId: "op_missing_scope",
      commandId: "profile.launch",
      status: "running",
      progress: 0.5,
    }),
});
await expectProblem(
  invalidOperationClient.reads.operation("op_missing_scope", controller.signal),
  "invalid_response",
  "cor_invalid_operation",
);
await expectProblem(
  client.reads.workspace("unknown", controller.signal),
  "workspace_scope_missing",
  "cor_self_test",
);
await expectProblem(
  client.writes.createOrganization(
    { organization, policy: { maxWorkspaces: 4, offlineEntitled: false } },
    { idempotencyKey: "short", signal: controller.signal },
  ),
  "idempotency_key_invalid",
  "cor_self_test",
);
assert(
  Number(requests.length) === 17,
  "Invalid idempotency key must fail before fetch",
);

const authRequests: Request[] = [];
const authClient = createWebApiClient({
  baseUrl: "http://127.0.0.1:47821",
  correlationId: () => "cor_auth",
  fetch: async (input, init) => {
    const request = new Request(input, init);
    authRequests.push(request);
    assert(
      request.credentials === "include",
      "Browser cookies must be included",
    );
    if (request.url.endsWith("/v1/auth/login"))
      return Response.json({ accountId: "acct_web", expiresAt: 1_800_000_000 });
    if (request.url.endsWith("/v1/auth/session"))
      return Response.json({ accountId: "acct_web", expiresAt: 1_800_000_000 });
    if (request.url.endsWith("/v1/auth/sessions"))
      return Response.json({
        sessions: [
          {
            id: "a".repeat(64),
            kind: "browser",
            deviceId: null,
            createdAt: 1_700_000_000,
            expiresAt: 1_800_000_000,
            revokedAt: null,
            current: true,
          },
        ],
      });
    if (request.method === "GET" && request.url.endsWith("/v1/auth/api-tokens"))
      return Response.json({
        tokens: [
          {
            id: "b".repeat(64),
            name: "automation",
            scopes: ["profile.read:read"],
            createdAt: 1_700_000_000,
            expiresAt: 1_800_000_000,
            revokedAt: null,
          },
        ],
      });
    if (
      request.method === "POST" &&
      request.url.endsWith("/v1/auth/api-tokens")
    )
      return Response.json(
        {
          token: `blapi_${"c".repeat(64)}`,
          tokenRecord: {
            id: "c".repeat(64),
            name: "automation",
            scopes: ["profile.read:read"],
            createdAt: 1_700_000_000,
            expiresAt: 1_800_000_000,
            revokedAt: null,
          },
        },
        { status: 201 },
      );
    if (
      request.method === "DELETE" &&
      request.url.includes("/v1/auth/api-tokens/")
    )
      return new Response(null, { status: 204 });
    if (request.url.endsWith("/v1/auth/recovery/request"))
      return new Response(null, { status: 202 });
    if (request.url.endsWith("/v1/auth/recovery/complete"))
      return new Response(null, { status: 204 });
    if (request.url.endsWith("/v1/auth/device/approve"))
      return Response.json({ approved: true });
    if (
      request.method === "DELETE" &&
      request.url.includes("/v1/auth/sessions/")
    )
      return new Response(null, { status: 204 });
    if (request.url.endsWith("/v1/auth/logout"))
      return new Response(null, { status: 204 });
    return Response.json({}, { status: 404 });
  },
});
const authSession = await authClient.auth.login(
  "owner",
  "correct horse battery staple",
  controller.signal,
);
assert(authSession.accountId === "acct_web", "Login session was not parsed");
assert(
  (await authClient.auth.session(controller.signal)).expiresAt ===
    1_800_000_000,
  "Session projection was not parsed",
);
assert(
  (await authClient.auth.sessions(controller.signal))[0].current,
  "Session list was not parsed",
);
assert(
  (await authClient.auth.apiTokens(controller.signal))[0].name === "automation",
  "API token list was not parsed",
);
assert(
  (
    await authClient.auth.createApiToken(
      {
        name: "automation",
        scopes: ["profile.read:read"],
        expiresAt: 1_800_000_000,
      },
      controller.signal,
    )
  ).token.startsWith("blapi_"),
  "API token create was not parsed",
);
await authClient.auth.revokeApiToken("b".repeat(64), controller.signal);
await authClient.auth.requestRecovery("owner@example.com", controller.signal);
await authClient.auth.completeRecovery(
  "d".repeat(64),
  "new correct horse battery staple",
  "recovery-complete-client-0001",
  controller.signal,
);
await authClient.auth.approveDevice("ABCD-EFGH", controller.signal);
await authClient.auth.revokeSession("a".repeat(64), controller.signal);
await authClient.auth.logout(controller.signal);
assert(
  authRequests.map((request) => request.method).join(",") ===
    "POST,GET,GET,GET,POST,DELETE,POST,POST,POST,DELETE,POST",
  "Auth method mismatch",
);
assert(
  authRequests[5].headers.get("idempotency-key") ===
    `api-token-revoke-${"b".repeat(24)}`,
  "API token revoke idempotency missing",
);
assert(
  authRequests[7].headers.get("idempotency-key") ===
    "recovery-complete-client-0001",
  "Recovery complete idempotency missing",
);
assert(
  authRequests[9].headers.get("idempotency-key") ===
    `session-revoke-${"a".repeat(24)}`,
  "Session revoke idempotency missing",
);
assert(
  !(await authRequests[0].text()).includes("accessToken"),
  "Login request must not contain bearer fields",
);

const profileController = new AbortController();
const profileRequests: Request[] = [];
const profile: Profile = {
  id: "prf_web_1",
  name: "Web profile",
  runtime: "wayfern",
  runtimeVersion: "1.0.0",
  runtimeConfig: null,
  state: "created",
  archived: false,
  fingerprint: null,
  note: null,
  proxyId: null,
  vpnId: null,
  groupIds: [],
  tags: [],
  extensionGroupId: null,
  syncMode: "local",
  createdAt: "2026-07-20T00:00:00Z",
  updatedAt: "2026-07-20T00:00:00Z",
};
const pendingProxyAssignment = {
  workspaceId: "ws_web_1",
  profileId: "prf_web_1",
  expectedProfileRevision: 1,
  proxyId: null,
  requestedAt: "2026-10-13T00:00:00Z",
};
const profileClient = createWebApiClient({
  accessToken: "profile-token",
  baseUrl: "http://127.0.0.1:47821",
  workspaceScopes: [{ organizationId: "org_web_1", workspaceId: "ws_web_1" }],
  correlationId: () => "cor_profile",
  fetch: async (input, init) => {
    const request = new Request(input, init);
    profileRequests.push(request);
    if (
      new URL(request.url).pathname === "/v1/profiles" &&
      request.method === "GET"
    ) {
      return new URL(request.url).searchParams.has("cursor")
        ? Response.json({ items: [], nextCursor: null })
        : Response.json({
            items: [{ schemaVersion: 1, revision: 1, value: profile }],
            nextCursor: "prf_web_1",
          });
    }
    if (request.url.endsWith("/v1/profiles") && request.method === "POST")
      return Response.json(
        { schemaVersion: 1, revision: 1, value: profile },
        { status: 201 },
      );
    if (request.url.endsWith("/v1/profiles/prf_web_1/launch"))
      return Response.json({
        operationId: "op_launch",
        commandId: "profile.launch",
        scope: {
          accountId: null,
          organizationId: null,
          workspaceId: "ws_web_1",
          teamId: null,
          memberId: null,
          resourceId: null,
          profileId: "prf_web_1",
        },
        status: "completed",
        progress: 1,
      });
    if (request.url.endsWith("/v1/profiles/prf_web_1/stop"))
      return Response.json({
        operationId: "op_stop",
        commandId: "profile.stop",
        scope: {
          accountId: null,
          organizationId: null,
          workspaceId: "ws_web_1",
          teamId: null,
          memberId: null,
          resourceId: null,
          profileId: "prf_web_1",
        },
        status: "completed",
        progress: 1,
      });
    if (request.url.endsWith("/v1/operations/op_launch"))
      return Response.json({
        operationId: "op_launch",
        commandId: "profile.launch",
        scope: {
          accountId: null,
          organizationId: null,
          workspaceId: "ws_web_1",
          teamId: null,
          memberId: null,
          resourceId: null,
          profileId: "prf_web_1",
        },
        status: "completed",
        progress: 1,
      });
    if (
      new URL(request.url).pathname ===
        "/v1/workspaces/ws_web_1/profiles/prf_web_1/proxy-assignment" &&
      ["GET", "PUT"].includes(request.method)
    )
      return Response.json(pendingProxyAssignment);
    if (
      new URL(request.url).pathname === "/v1/workspaces/ws_web_1/groups" &&
      request.method === "GET"
    )
      return Response.json([
        {
          group: {
            id: "grp_web_1",
            name: "Social",
            appearance: { color: "blue" },
            createdAt: "2026-10-13T00:00:00Z",
            updatedAt: "2026-10-13T00:00:00Z",
          },
          workspaceId: "ws_web_1",
          revision: 1,
        },
      ]);
    if (
      new URL(request.url).pathname ===
        "/v1/workspaces/ws_web_1/groups/grp_web_1" &&
      request.method === "GET"
    )
      return Response.json({
        group: {
          id: "grp_web_1",
          name: "Social",
          appearance: { color: "blue" },
          createdAt: "2026-10-13T00:00:00Z",
          updatedAt: "2026-10-13T00:00:00Z",
        },
        workspaceId: "ws_web_1",
        revision: 1,
      });
    if (
      new URL(request.url).pathname === "/v1/workspaces/ws_web_1/groups" &&
      request.method === "POST"
    )
      return Response.json({
        group: {
          id: "grp_web_1",
          name: "Social",
          appearance: null,
          createdAt: "2026-10-13T00:00:00Z",
          updatedAt: "2026-10-13T00:00:00Z",
        },
        workspaceId: "ws_web_1",
        revision: 1,
      });
    if (
      new URL(request.url).pathname ===
        "/v1/workspaces/ws_web_1/groups/grp_web_1" &&
      ["PUT", "DELETE"].includes(request.method)
    )
      return Response.json({
        group: {
          id: "grp_web_1",
          name: "Social",
          appearance: null,
          createdAt: "2026-10-13T00:00:00Z",
          updatedAt: "2026-10-13T00:00:00Z",
        },
        workspaceId: "ws_web_1",
        revision: 2,
      });
    if (
      new URL(request.url).pathname === "/v1/profiles/prf_web_1/groups" &&
      request.method === "PUT"
    )
      return Response.json({
        ...profile,
        groupIds: ["grp_web_1"],
        revision: 2,
      });
    if (request.url.endsWith("/v1/proxies/parse") && request.method === "POST")
      return Response.json({
        validCount: 1,
        invalidCount: 1,
        duplicateCount: 0,
        rows: [
          { protocol: "http", endpoint: "proxy.example:8080", valid: true },
          {
            protocol: null,
            endpoint: null,
            valid: false,
            line: 2,
            error: "invalid_port",
          },
        ],
      });
    return Response.json({}, { status: 404 });
  },
});

const operationalClient = createWebApiClient({
  accessToken: "workspace-token",
  baseUrl: "http://127.0.0.1:47821",
  workspaceScopes: [{ organizationId: "org:1", workspaceId: "workspace:1" }],
  fetch: async (input) => {
    const url = new URL(String(input));
    if (url.pathname === "/v1/organizations/org%3A1/workspaces/workspace%3A1")
      return Response.json(workspace);
    if (url.pathname === "/v1/workspaces/workspace%3A1/teams")
      return Response.json([{ team, revision: 3 }]);
    if (url.pathname === "/v1/workspaces/workspace%3A1/members")
      return Response.json([]);
    if (url.pathname === "/v1/workspaces/workspace%3A1/resources")
      return Response.json([]);
    if (url.pathname === "/v1/workspaces/workspace%3A1/grants")
      return Response.json([]);
    return Response.json({}, { status: 404 });
  },
});
const operational = await operationalClient.reads.workspaceOperational(
  "workspace:1",
  profileController.signal,
);
assert(
  operational.teams.length === 1 && operational.teams[0].id === team.id,
  "Versioned REST team records were not projected into the workspace view",
);

const profiles = await profileClient.reads.profiles(
  null,
  50,
  profileController.signal,
);
assert(
  profiles.items[0].value.id === "prf_web_1" &&
    profiles.nextCursor === "prf_web_1",
  "Profile page response was not parsed",
);
const createdProfile = await profileClient.writes.createProfile(
  {
    id: profile.id,
    name: profile.name,
    runtime: profile.runtime,
    runtimeVersion: profile.runtimeVersion ?? undefined,
    workspaceId: "ws_web_1",
    proxyId: profile.proxyId ?? undefined,
    note: "Created by client",
  },
  {
    idempotencyKey: "profile-create-self-0001",
    signal: profileController.signal,
  },
);
assert(createdProfile.revision === 1, "Profile create response was not parsed");
const launched = await profileClient.writes.launchProfile(
  "prf_web_1",
  { profilesRoot: "C:/BugLogin/profiles", extraArgs: [] },
  {
    idempotencyKey: "profile-launch-self-0001",
    signal: profileController.signal,
  },
);
assert(
  launched.status === "completed",
  "Profile launch operation was not parsed",
);
const operation = await profileClient.reads.operation(
  "op_launch",
  profileController.signal,
);
assert(
  operation.operationId === "op_launch",
  "Operation response was not parsed",
);
await profileClient.writes.stopProfile("prf_web_1", {
  idempotencyKey: "profile-stop-self-0001",
  signal: profileController.signal,
});
const proxyReport = await profileClient.writes.parseProxies(
  "http://proxy.example:8080\ninvalid",
  profileController.signal,
);
assert(
  proxyReport.validCount === 1 &&
    proxyReport.invalidCount === 1 &&
    proxyReport.rows[1].error === "invalid_port",
  "Proxy parse response was not parsed",
);
const nextProfilePage = await profileClient.reads.profiles(
  profiles.nextCursor,
  50,
  profileController.signal,
);
assert(
  nextProfilePage.items.length === 0 && nextProfilePage.nextCursor === null,
  "Profile next page response was not parsed",
);
const stagedProxyAssignment =
  await profileClient.writes.stageProfileProxyAssignment(
    {
      workspaceId: "ws_web_1",
      profileId: "prf_web_1",
      expectedProfileRevision: 1,
      proxyId: null,
    },
    {
      idempotencyKey: "profile-proxy-assignment-0001",
      signal: profileController.signal,
    },
  );
assert(
  stagedProxyAssignment.proxyId === null &&
    stagedProxyAssignment.expectedProfileRevision === 1,
  "Profile proxy assignment response was not parsed",
);
const loadedProxyAssignment = await profileClient.reads.profileProxyAssignment(
  { workspaceId: "ws_web_1", profileId: "prf_web_1" },
  profileController.signal,
);
assert(
  loadedProxyAssignment.profileId === "prf_web_1" &&
    loadedProxyAssignment.proxyId === null,
  "Pending profile proxy assignment was not loaded",
);
assert(profileRequests[0].method === "GET", "Profile list method mismatch");
assert(
  profileRequests[1].headers.get("idempotency-key") ===
    "profile-create-self-0001",
  "Profile create idempotency missing",
);
const createProfileBody = await profileRequests[1].clone().json();
assert(
  createProfileBody.id === profile.id &&
    createProfileBody.workspaceId === "ws_web_1" &&
    createProfileBody.note === "Created by client",
  "Profile create DTO did not preserve caller fields",
);
assert(
  profileRequests[2].headers.get("idempotency-key") ===
    "profile-launch-self-0001",
  "Profile launch idempotency missing",
);
assert(
  profileRequests[5].headers.get("content-type")?.startsWith("text/plain"),
  "Proxy parse must send text/plain",
);
assert(
  (await profileRequests[5].text()) === "http://proxy.example:8080\ninvalid",
  "Proxy parse body mismatch",
);
assert(
  new URL(profileRequests[6].url).searchParams.get("cursor") === "prf_web_1",
  "Profile cursor was not sent",
);
assert(
  new URL(profileRequests[0].url).searchParams.get("limit") === "50",
  "Profile page limit was not sent",
);
assert(
  profileRequests[7].method === "PUT" &&
    profileRequests[7].headers.get("idempotency-key") ===
      "profile-proxy-assignment-0001",
  "Profile proxy assignment mutation must use PUT and idempotency",
);
assert(
  (await profileRequests[7].clone().json()).proxyId === null,
  "Profile proxy unassignment must send an explicit null value",
);
assert(
  profileRequests[8].method === "GET",
  "Profile proxy assignment read method mismatch",
);
const groupRecords = await profileClient.reads.groups(
  "ws_web_1",
  profileController.signal,
);
assert(
  groupRecords.length === 1 && groupRecords[0].group.id === "grp_web_1",
  "Workspace groups response was not parsed",
);
const groupRecord = await profileClient.reads.group(
  "ws_web_1",
  "grp_web_1",
  profileController.signal,
);
assert(groupRecord.revision === 1, "Group response revision was not parsed");
const createdGroup = await profileClient.writes.createGroup(
  {
    workspaceId: "ws_web_1",
    groupId: "grp_web_1",
    name: "Social",
    appearance: null,
  },
  { idempotencyKey: "group-create-web-0001", signal: profileController.signal },
);
assert(
  createdGroup.group.name === "Social" && createdGroup.revision === 1,
  "Group create response was not parsed",
);
const updatedGroup = await profileClient.writes.updateGroup(
  {
    workspaceId: "ws_web_1",
    groupId: "grp_web_1",
    expectedRevision: 1,
    name: "Social",
    appearance: null,
  },
  { idempotencyKey: "group-update-web-0001", signal: profileController.signal },
);
assert(updatedGroup.revision === 2, "Group update response was not parsed");
const deletedGroup = await profileClient.writes.deleteGroup(
  { workspaceId: "ws_web_1", groupId: "grp_web_1", expectedRevision: 2 },
  { idempotencyKey: "group-delete-web-0001", signal: profileController.signal },
);
assert(
  deletedGroup.group.id === "grp_web_1",
  "Group delete response was not parsed",
);
const groupedProfile = await profileClient.writes.replaceProfileGroups(
  {
    workspaceId: "ws_web_1",
    profileId: "prf_web_1",
    expectedRevision: 1,
    groupIds: ["grp_web_1"],
  },
  {
    idempotencyKey: "profile-groups-web-0001",
    signal: profileController.signal,
  },
);
assert(
  groupedProfile.revision === 2 && groupedProfile.groupIds[0] === "grp_web_1",
  "Profile group replacement response was not parsed",
);
assert(
  profileRequests
    .slice(-6)
    .map((request) => request.method)
    .join(",") === "GET,GET,POST,PUT,DELETE,PUT",
  "Group API methods mismatch",
);
const lastProfileRequest = profileRequests.at(-1);
if (!lastProfileRequest)
  throw new Error("Profile group replacement request was not captured");
assert(
  lastProfileRequest.headers.get("idempotency-key") ===
    "profile-groups-web-0001",
  "Profile group replacement idempotency missing",
);
assert(
  (await lastProfileRequest.clone().json()).expectedRevision === 1,
  "Profile group replacement must send the expected revision",
);

async function expectProblem(
  promise: Promise<unknown>,
  code: string,
  correlationId: string,
): Promise<void> {
  try {
    await promise;
    throw new Error(`Expected ${code}`);
  } catch (error) {
    assert(error instanceof WebApiError, "Expected WebApiError");
    assert(
      error.problem.code === code,
      `Expected ${code}, received ${error.problem.code}`,
    );
    assert(
      error.problem.correlationId === correlationId,
      "Problem correlation mismatch",
    );
  }
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

import { WebApiError, type Profile } from './api.ts';
import { createWebApiClient } from './http-client.ts';

const controller = new AbortController();
const requests: Request[] = [];
const organization = { id: 'org:2', accountId: 'account-1', name: 'Operations', createdAt: '2026-07-22T00:00:00Z' };
const workspace = { id: 'workspace:1', organizationId: 'org:1', name: 'Production', policy: { maxProfiles: 12, maxMembers: 8, maxStorageBytes: 4096, offlineEntitled: true }, createdAt: '2026-07-20T00:00:00Z' };
const team = { id: 'team:1', workspaceId: 'workspace:1', name: 'Operators', memberIds: [] };
const invitedMember = { id: 'member:1', accountId: 'account-2', workspaceId: 'workspace:1', teamId: 'team:1', role: 'member' as const, status: 'invited' as const, inviteExpiresAt: 1_800_000_000 };
const resourceGrant = { id: 'grant:1', workspaceId: 'workspace:1', resourceId: 'resource:1', memberId: 'member:1', action: 'launch', effect: 'allow' };
const client = createWebApiClient({
  accessToken: 'secret-token',
  baseUrl: 'http://127.0.0.1:47821/',
  correlationId: () => 'cor_self_test',
  workspaceScopes: [{ organizationId: 'org:1', workspaceId: 'workspace:1' }],
  fetch: async (input, init) => {
    assert(init?.signal === controller.signal, 'AbortSignal was not forwarded');
    const request = new Request(input, init);
    requests.push(request);
    assert(!request.url.includes('secret-token'), 'Token leaked into URL');
    assert(request.headers.get('authorization') === 'Bearer secret-token', 'Bearer header missing');
    assert(request.headers.get('x-correlation-id') === 'cor_self_test', 'Correlation header missing');
    if (request.method === 'GET' && request.url.endsWith('/v1/health')) return Response.json({ status: 'ok', service: 'buglogin-api' });
    if (request.method === 'GET' && request.url.endsWith('/v1/account')) return Response.json({ accountId: 'account-1' });
    if (request.method === 'GET' && request.url.endsWith('/v1/organizations')) return Response.json([{ id: 'org:1', accountId: 'account-1', name: 'Production', createdAt: '2026-07-20T00:00:00Z' }]);
    if (request.method === 'GET' && request.url.endsWith('/v1/organizations/org%3A1/workspaces')) return Response.json([{ id: 'workspace:1', organizationId: 'org:1', name: 'Production', policy: { maxProfiles: 12, maxMembers: 8, maxStorageBytes: 4096, offlineEntitled: true }, createdAt: '2026-07-20T00:00:00Z' }]);
    if (request.method === 'GET' && request.url.endsWith('/v1/organizations/org%3A1/workspaces/workspace%3A1')) return Response.json(workspace);
    if (request.method === 'POST' && request.url.endsWith('/v1/organizations')) return Response.json(null);
    if (request.method === 'PATCH' && request.url.endsWith('/v1/organizations/org%3A2')) return Response.json(organization);
    if (request.method === 'POST' && request.url.endsWith('/v1/organizations/org%3A1/workspaces')) return Response.json(workspace);
    if (request.method === 'PATCH' && request.url.endsWith('/v1/organizations/org%3A1/workspaces/workspace%3A1')) return Response.json(workspace);
    if (request.method === 'POST' && request.url.endsWith('/v1/workspaces/workspace%3A1/teams')) return Response.json({ team, revision: 1 });
    if (request.method === 'PUT' && request.url.endsWith('/v1/workspaces/workspace%3A1/teams/team%3A1')) return Response.json({ team: { ...team, name: 'Platform' }, revision: 2 });
    if (request.method === 'DELETE' && request.url.endsWith('/v1/workspaces/workspace%3A1/teams/team%3A1')) return Response.json({ team, revision: 2 });
    if (request.method === 'POST' && request.url.endsWith('/v1/workspaces/workspace%3A1/members')) return Response.json(invitedMember);
    if (request.method === 'PUT' && request.url.endsWith('/activate')) return Response.json({ ...invitedMember, status: 'active', inviteExpiresAt: null });
    if (request.method === 'PUT' && request.url.endsWith('/suspend')) return Response.json({ ...invitedMember, status: 'suspended', inviteExpiresAt: null });
    if (request.method === 'PUT' && request.url.endsWith('/role')) return Response.json({ ...invitedMember, role: 'admin' });
    if (request.method === 'POST' && request.url.endsWith('/v1/workspaces/workspace%3A1/grants')) return Response.json(resourceGrant);
    if (request.method === 'DELETE' && request.url.endsWith('/v1/workspaces/workspace%3A1/grants/grant%3A1')) return Response.json({ grantId: 'grant:1', revoked: true });
    return Response.json({}, { status: 404 });
  },
});

const summary = await client.reads.publicSummary(controller.signal);
assert(summary.status.label === 'Operational', 'Health response was not mapped');
const account = await client.reads.account(controller.signal);
assert(account.accountId === 'account-1', 'Account response was not parsed');
const workspaces = await client.reads.workspaces(controller.signal);
assert(workspaces.length === 1 && workspaces[0].policy.maxMembers === 8, 'Workspace list was not parsed');
const workspaceResult = await client.reads.workspace('workspace:1', controller.signal);
assert(workspaceResult.organizationId === 'org:1', 'Workspace detail was not parsed');
assert(requests.length === 4, 'Unexpected fetch count');
assert(requests.every(request => request.method === 'GET'), 'Read method mismatch');

const mutationOptions = { idempotencyKey: 'idem-self-test-0001', signal: controller.signal };
await client.writes.createOrganization({ organization, policy: { maxWorkspaces: 4, offlineEntitled: false } }, mutationOptions);
const updatedOrganization = await client.writes.updateOrganization(organization.id, { organization, policy: { maxWorkspaces: 4, offlineEntitled: false }, expectedRevision: 1 }, mutationOptions);
assert(updatedOrganization.name === 'Operations', 'Organization update response was not parsed');
await client.writes.createWorkspace('org:1', workspace, mutationOptions);
await client.writes.updateWorkspace(workspace.id, { workspace, expectedRevision: 1 }, mutationOptions);
const createdTeam = await client.writes.createTeam(workspace.id, team, mutationOptions);
assert(createdTeam.revision === 1, 'Team create revision was not parsed');
const updatedTeam = await client.writes.updateTeam(workspace.id, team.id, { name: 'Platform', expectedRevision: 1 }, mutationOptions);
assert(updatedTeam.team.name === 'Platform' && updatedTeam.revision === 2, 'Team update response was not parsed');
const deletedTeam = await client.writes.deleteTeam(workspace.id, team.id, { expectedRevision: 2 }, mutationOptions);
assert(deletedTeam.team.id === team.id, 'Team delete response was not parsed');
await client.writes.inviteMember(workspace.id, invitedMember, mutationOptions);
const activatedMember = await client.writes.activateMember(workspace.id, invitedMember.id, mutationOptions);
assert(activatedMember.status === 'active', 'Member activate response was not parsed');
const suspendedMember = await client.writes.suspendMember(workspace.id, invitedMember.id, mutationOptions);
assert(suspendedMember.status === 'suspended', 'Member suspend response was not parsed');
const promotedMember = await client.writes.updateMemberRole(workspace.id, invitedMember.id, { role: 'admin' }, mutationOptions);
assert(promotedMember.role === 'admin', 'Member role response was not parsed');
const createdGrant = await client.writes.createGrant(workspace.id, resourceGrant, mutationOptions);
assert(createdGrant.id === resourceGrant.id, 'Grant create response was not parsed');
const revokedGrant = await client.writes.revokeGrant(workspace.id, resourceGrant.id, mutationOptions);
assert(revokedGrant.revoked && revokedGrant.grantId === resourceGrant.id, 'Grant revoke response was not parsed');

const mutationRequests = requests.slice(4);
assert(mutationRequests.length === 13, 'Unexpected mutation fetch count');
assert(mutationRequests.every(request => request.headers.get('idempotency-key') === mutationOptions.idempotencyKey), 'Idempotency header missing');
assert(mutationRequests.every(request => request.headers.get('authorization') === 'Bearer secret-token'), 'Mutation bearer header missing');
assert(mutationRequests.every(request => request.headers.get('x-correlation-id') === 'cor_self_test'), 'Mutation correlation header missing');
assert(mutationRequests.map(request => request.method).join(',') === 'POST,PATCH,POST,PATCH,POST,PUT,DELETE,POST,PUT,PUT,PUT,POST,DELETE', 'Mutation method mismatch');
assert(await mutationRequests[1].json().then(value => value.expectedRevision === 1), 'Organization revision body missing');
assert(await mutationRequests[5].json().then(value => value.expectedRevision === 1), 'Team revision body missing');
assert(await mutationRequests[6].json().then(value => value.expectedRevision === 2), 'Team delete revision body missing');
assert(mutationRequests[8].headers.get('content-type') === null && (await mutationRequests[8].text()) === '', 'Activate must not send a body');
assert(mutationRequests[9].headers.get('content-type') === null && (await mutationRequests[9].text()) === '', 'Suspend must not send a body');
assert(await mutationRequests[11].json().then(value => value.id === resourceGrant.id), 'Grant create body missing');
assert(mutationRequests[12].url.endsWith('/v1/workspaces/workspace%3A1/grants/grant%3A1'), 'Grant revoke path mismatch');

const problemClient = createWebApiClient({
  baseUrl: 'http://127.0.0.1:47821',
  correlationId: () => 'cor_request',
  fetch: async () => Response.json({
    type: 'https://buglogin.local/problems/authentication_required',
    title: 'Authentication required',
    status: 401,
    code: 'authentication_required',
    correlationId: 'cor_server',
    retryable: false,
    fields: {},
  }, { status: 401 }),
});

await expectProblem(problemClient.reads.publicSummary(controller.signal), 'authentication_required', 'cor_server');
await expectProblem(problemClient.writes.createOrganization({ organization, policy: { maxWorkspaces: 4, offlineEntitled: false } }, mutationOptions), 'authentication_required', 'cor_server');
assert(Number(requests.length) === 17, 'Problem client must not affect primary request log');

const invalidClient = createWebApiClient({
  baseUrl: 'http://127.0.0.1:47821',
  correlationId: () => 'cor_invalid',
  fetch: async () => Response.json({ accountId: 1 }),
});
await expectProblem(invalidClient.reads.account(controller.signal), 'invalid_response', 'cor_invalid');
await expectProblem(client.reads.workspace('unknown', controller.signal), 'workspace_scope_missing', 'cor_self_test');
await expectProblem(client.writes.createOrganization({ organization, policy: { maxWorkspaces: 4, offlineEntitled: false } }, { idempotencyKey: 'short', signal: controller.signal }), 'idempotency_key_invalid', 'cor_self_test');
assert(Number(requests.length) === 17, 'Invalid idempotency key must fail before fetch');

const authRequests: Request[] = [];
const authClient = createWebApiClient({
  baseUrl: 'http://127.0.0.1:47821',
  correlationId: () => 'cor_auth',
  fetch: async (input, init) => {
    const request = new Request(input, init);
    authRequests.push(request);
    assert(request.credentials === 'include', 'Browser cookies must be included');
    if (request.url.endsWith('/v1/auth/login')) return Response.json({ accountId: 'acct_web', expiresAt: 1_800_000_000 });
    if (request.url.endsWith('/v1/auth/session')) return Response.json({ accountId: 'acct_web', expiresAt: 1_800_000_000 });
    if (request.url.endsWith('/v1/auth/sessions')) return Response.json({ sessions: [{ id: 'a'.repeat(64), kind: 'browser', deviceId: null, createdAt: 1_700_000_000, expiresAt: 1_800_000_000, revokedAt: null, current: true }] });
    if (request.method === 'GET' && request.url.endsWith('/v1/auth/api-tokens')) return Response.json({ tokens: [{ id: 'b'.repeat(64), name: 'automation', scopes: ['profile.read:read'], createdAt: 1_700_000_000, expiresAt: 1_800_000_000, revokedAt: null }] });
    if (request.method === 'POST' && request.url.endsWith('/v1/auth/api-tokens')) return Response.json({ token: `blapi_${'c'.repeat(64)}`, tokenRecord: { id: 'c'.repeat(64), name: 'automation', scopes: ['profile.read:read'], createdAt: 1_700_000_000, expiresAt: 1_800_000_000, revokedAt: null } }, { status: 201 });
    if (request.method === 'DELETE' && request.url.includes('/v1/auth/api-tokens/')) return new Response(null, { status: 204 });
    if (request.url.endsWith('/v1/auth/recovery/request')) return new Response(null, { status: 202 });
    if (request.url.endsWith('/v1/auth/recovery/complete')) return new Response(null, { status: 204 });
    if (request.url.endsWith('/v1/auth/device/approve')) return Response.json({ approved: true });
    if (request.method === 'DELETE' && request.url.includes('/v1/auth/sessions/')) return new Response(null, { status: 204 });
    if (request.url.endsWith('/v1/auth/logout')) return new Response(null, { status: 204 });
    return Response.json({}, { status: 404 });
  },
});
const authSession = await authClient.auth.login('owner', 'correct horse battery staple', controller.signal);
assert(authSession.accountId === 'acct_web', 'Login session was not parsed');
assert((await authClient.auth.session(controller.signal)).expiresAt === 1_800_000_000, 'Session projection was not parsed');
assert((await authClient.auth.sessions(controller.signal))[0].current, 'Session list was not parsed');
assert((await authClient.auth.apiTokens(controller.signal))[0].name === 'automation', 'API token list was not parsed');
assert((await authClient.auth.createApiToken({ name: 'automation', scopes: ['profile.read:read'], expiresAt: 1_800_000_000 }, controller.signal)).token.startsWith('blapi_'), 'API token create was not parsed');
await authClient.auth.revokeApiToken('b'.repeat(64), controller.signal);
await authClient.auth.requestRecovery('owner@example.com', controller.signal);
await authClient.auth.completeRecovery('d'.repeat(64), 'new correct horse battery staple', 'recovery-complete-client-0001', controller.signal);
await authClient.auth.approveDevice('ABCD-EFGH', controller.signal);
await authClient.auth.revokeSession('a'.repeat(64), controller.signal);
await authClient.auth.logout(controller.signal);
assert(authRequests.map(request => request.method).join(',') === 'POST,GET,GET,GET,POST,DELETE,POST,POST,POST,DELETE,POST', 'Auth method mismatch');
assert(authRequests[5].headers.get('idempotency-key') === `api-token-revoke-${'b'.repeat(24)}`, 'API token revoke idempotency missing');
assert(authRequests[7].headers.get('idempotency-key') === 'recovery-complete-client-0001', 'Recovery complete idempotency missing');
assert(authRequests[9].headers.get('idempotency-key') === `session-revoke-${'a'.repeat(24)}`, 'Session revoke idempotency missing');
assert(!(await authRequests[0].text()).includes('accessToken'), 'Login request must not contain bearer fields');

const profileController = new AbortController();
const profileRequests: Request[] = [];
const profile: Profile = { id: 'prf_web_1', name: 'Web profile', runtime: 'wayfern', runtimeVersion: '1.0.0', runtimeConfig: null, state: 'created', archived: false, fingerprint: null, note: null, proxyId: null, vpnId: null, groupIds: [], tags: [], extensionGroupId: null, syncMode: 'local', createdAt: '2026-07-20T00:00:00Z', updatedAt: '2026-07-20T00:00:00Z' };
const profileClient = createWebApiClient({
  accessToken: 'profile-token',
  baseUrl: 'http://127.0.0.1:47821',
  correlationId: () => 'cor_profile',
  fetch: async (input, init) => {
    const request = new Request(input, init);
    profileRequests.push(request);
    if (request.url.endsWith('/v1/profiles') && request.method === 'GET') return Response.json([{ revision: 1, value: profile }]);
    if (request.url.endsWith('/v1/profiles') && request.method === 'POST') return Response.json({ revision: 1, value: profile }, { status: 201 });
    if (request.url.endsWith('/v1/profiles/prf_web_1/launch')) return Response.json({ operationId: 'op_launch', commandId: 'profile.launch', scope: {}, status: 'completed', progress: 1 });
    if (request.url.endsWith('/v1/profiles/prf_web_1/stop')) return Response.json({ operationId: 'op_stop', commandId: 'profile.stop', scope: {}, status: 'completed', progress: 1 });
    if (request.url.endsWith('/v1/operations/op_launch')) return Response.json({ operationId: 'op_launch', commandId: 'profile.launch', scope: {}, status: 'completed', progress: 1 });
    if (request.url.endsWith('/v1/proxies/parse') && request.method === 'POST') return Response.json({ validCount: 1, invalidCount: 1, duplicateCount: 0, rows: [{ protocol: 'http', endpoint: 'proxy.example:8080', valid: true }, { protocol: null, endpoint: null, valid: false, line: 2, error: 'invalid_port' }] });
    return Response.json({}, { status: 404 });
  },
});
const profiles = await profileClient.reads.profiles(profileController.signal);
assert(profiles[0].value.id === 'prf_web_1', 'Profile list response was not parsed');
const createdProfile = await profileClient.writes.createProfile({ name: profile.name, runtime: profile.runtime, runtimeVersion: profile.runtimeVersion ?? undefined, workspaceId: 'ws_web_1', proxyId: profile.proxyId ?? undefined }, { idempotencyKey: 'profile-create-self-0001', signal: profileController.signal });
assert(createdProfile.revision === 1, 'Profile create response was not parsed');
const launched = await profileClient.writes.launchProfile('prf_web_1', { profilesRoot: 'C:/BugLogin/profiles', extraArgs: [] }, { idempotencyKey: 'profile-launch-self-0001', signal: profileController.signal });
assert(launched.status === 'completed', 'Profile launch operation was not parsed');
const operation = await profileClient.reads.operation('op_launch', profileController.signal);
assert(operation.operationId === 'op_launch', 'Operation response was not parsed');
await profileClient.writes.stopProfile('prf_web_1', { idempotencyKey: 'profile-stop-self-0001', signal: profileController.signal });
const proxyReport = await profileClient.writes.parseProxies('http://proxy.example:8080\ninvalid', profileController.signal);
assert(proxyReport.validCount === 1 && proxyReport.invalidCount === 1 && proxyReport.rows[1].error === 'invalid_port', 'Proxy parse response was not parsed');
assert(profileRequests[0].method === 'GET', 'Profile list method mismatch');
assert(profileRequests[1].headers.get('idempotency-key') === 'profile-create-self-0001', 'Profile create idempotency missing');
assert(profileRequests[2].headers.get('idempotency-key') === 'profile-launch-self-0001', 'Profile launch idempotency missing');
assert(profileRequests[5].headers.get('content-type')?.startsWith('text/plain'), 'Proxy parse must send text/plain');
assert((await profileRequests[5].text()) === 'http://proxy.example:8080\ninvalid', 'Proxy parse body mismatch');

async function expectProblem(promise: Promise<unknown>, code: string, correlationId: string): Promise<void> {
  try {
    await promise;
    throw new Error(`Expected ${code}`);
  } catch (error) {
    assert(error instanceof WebApiError, 'Expected WebApiError');
    assert(error.problem.code === code, `Expected ${code}, received ${error.problem.code}`);
    assert(error.problem.correlationId === correlationId, 'Problem correlation mismatch');
  }
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

import {
  WebApiError,
  type AccountSummary,
  type ApiTokenCreateInput,
  type ApiTokenSummary,
  type AdminSummary,
  type LaunchInput,
  type MigrationImportInput,
  type MigrationInspectInput,
  type Operation,
  type OperationStatus,
  type Profile,
  type ProfileCreateInput,
  type ProfileState,
  type ProxyParseReport,
  type ProxyView,
  type ProxyPage,
  type ProxyRecord,
  type ProxyCreateInput,
  type ProxyCreateResult,
  type ProxyUpdateInput,
  type ProxyDeleteInput,
  type Runtime,
  type SyncMode,
  type GrantSummary,
  type GrantCreateInput,
  type GrantRevocationReceipt,
  type MemberSummary,
  type MutationOptions,
  type OrganizationSummary,
  type Problem,
  type PublicSummary,
  type ResourceSummary,
  type TeamRecord,
  type TeamDeleteInput,
  type TeamSummary,
  type WebApiClient,
  type WebSession,
  type WebSessionSummary,
  type WorkspaceOperationalData,
  type WorkspaceSummary,
  type VersionedProfile,
} from './api.ts';

export type WorkspaceScope = { organizationId: string; workspaceId: string };
export type WebApiClientOptions = {
  baseUrl: string;
  accessToken?: string;
  workspaceScopes?: readonly WorkspaceScope[];
  fetch?: typeof fetch;
  correlationId?: () => string;
};

type Health = { status: 'ok'; service?: string; contractVersion?: string };
type RequestOptions = { allowEmpty?: boolean; body?: unknown; textBody?: string; idempotencyKey?: string; method?: 'DELETE' | 'GET' | 'PATCH' | 'POST' | 'PUT'; signal: AbortSignal };

export function createWebApiClient(options: WebApiClientOptions): WebApiClient {
  const fetchRequest = options.fetch ?? globalThis.fetch;
  const baseUrl = options.baseUrl.replace(/\/$/, '');
  const nextCorrelationId = options.correlationId ?? (() => `cor_${globalThis.crypto.randomUUID()}`);
  const workspaceScopes = options.workspaceScopes ?? [];

  async function request<T>(path: string, requestOptions: RequestOptions, parse: (value: unknown) => T | undefined): Promise<T> {
    const correlationId = nextCorrelationId();
    const headers = new Headers({ Accept: 'application/json, application/problem+json', 'X-Correlation-Id': correlationId });
    if (options.accessToken) headers.set('Authorization', `Bearer ${options.accessToken}`);
    const method = requestOptions.method ?? 'GET';
    if (requestOptions.idempotencyKey) headers.set('Idempotency-Key', validIdempotencyKey(requestOptions.idempotencyKey, correlationId));
    if (requestOptions.body !== undefined) headers.set('Content-Type', 'application/json');
    if (requestOptions.textBody !== undefined) headers.set('Content-Type', 'text/plain; charset=utf-8');
    if (method !== 'GET' && !options.accessToken && typeof document !== 'undefined') {
      const csrf = document.cookie.split(';').map(value => value.trim()).find(value => value.startsWith('__Host-buglogin_csrf='))?.split('=').slice(1).join('=');
      if (csrf) headers.set('X-CSRF-Token', csrf);
    }
    const response = await fetchRequest(`${baseUrl}${path}`, {
      credentials: 'include',
      headers,
      method,
      signal: requestOptions.signal,
      ...(requestOptions.textBody !== undefined ? { body: requestOptions.textBody } : requestOptions.body === undefined ? {} : { body: JSON.stringify(requestOptions.body) }),
    });
    if (!response.ok) throw new WebApiError(await responseProblem(response, correlationId));
    if (requestOptions.allowEmpty) return undefined as T;
    try {
      const parsed = parse(await response.json());
      if (parsed !== undefined) return parsed;
    } catch {}
    throw new WebApiError(problem('invalid_response', 'Invalid API response', 502, correlationId));
  }

  function scopeFor(id: string): WorkspaceScope {
    const matches = workspaceScopes.filter(scope => scope.workspaceId === id);
    if (matches.length !== 1) throw new WebApiError(problem('workspace_scope_missing', 'Workspace organization scope is missing', 400, nextCorrelationId()));
    return matches[0];
  }

  async function organizations(signal: AbortSignal): Promise<OrganizationSummary[]> {
    return request('/v1/organizations', { signal }, value => parseArray(value, parseOrganization));
  }

  async function workspaces(signal: AbortSignal): Promise<WorkspaceSummary[]> {
    const organizationIds = [...new Set(workspaceScopes.map(scope => scope.organizationId))];
    if (organizationIds.length === 0) throw new WebApiError(problem('workspace_scope_missing', 'Workspace organization scope is missing', 400, nextCorrelationId()));
    const pages = await Promise.all(organizationIds.map(id => request(`/v1/organizations/${encodeURIComponent(id)}/workspaces`, { signal }, value => parseArray(value, parseWorkspace))));
    const allowed = new Set(workspaceScopes.map(scope => scope.workspaceId));
    return pages.flat().filter(workspace => allowed.has(workspace.id));
  }

  async function workspaceOperational(id: string, signal: AbortSignal): Promise<WorkspaceOperationalData> {
    const scope = scopeFor(id);
    const workspaceBase = `/v1/workspaces/${encodeURIComponent(id)}`;
    const [workspace, teams, members, resources, grants] = await Promise.all([
      request(workspacePath(scope), { signal }, parseWorkspace),
      request(`${workspaceBase}/teams`, { signal }, value => parseArray(value, parseTeam)),
      request(`${workspaceBase}/members`, { signal }, value => parseArray(value, parseMember)),
      request(`${workspaceBase}/resources`, { signal }, value => parseArray(value, parseResource)),
      request(`${workspaceBase}/grants`, { signal }, value => parseArray(value, parseGrant)),
    ]);
    return { workspace, teams, members, resources, grants };
  }

  return {
    auth: {
      login: (identifier: string, password: string, signal: AbortSignal) => request('/v1/auth/login', { body: { identifier, password }, method: 'POST', signal }, parseWebSession),
      session: (signal: AbortSignal) => request('/v1/auth/session', { signal }, parseWebSession),
      sessions: (signal: AbortSignal) => request('/v1/auth/sessions', { signal }, value => isRecord(value) && Array.isArray(value.sessions) ? value.sessions.map(parseWebSessionSummary).filter((item): item is WebSessionSummary => item !== undefined) : undefined),
      apiTokens: signal => request('/v1/auth/api-tokens', { signal }, value => isRecord(value) && Array.isArray(value.tokens) ? value.tokens.map(parseApiToken).filter((item): item is ApiTokenSummary => item !== undefined) : undefined),
      createApiToken: (input: ApiTokenCreateInput, signal: AbortSignal) => request('/v1/auth/api-tokens', { body: input, method: 'POST', signal }, value => isRecord(value) && typeof value.token === 'string' && parseApiToken(value.tokenRecord) ? { token: value.token, tokenRecord: parseApiToken(value.tokenRecord)! } : undefined),
      revokeApiToken: (tokenId: string, signal: AbortSignal) => request(`/v1/auth/api-tokens/${encodeURIComponent(tokenId)}`, { allowEmpty: true, idempotencyKey: `api-token-revoke-${tokenId.slice(0, 24)}`, method: 'DELETE', signal }, () => undefined) as Promise<void>,
      requestRecovery: (identifier: string, signal: AbortSignal) => request('/v1/auth/recovery/request', { allowEmpty: true, body: { identifier }, method: 'POST', signal }, () => undefined) as Promise<void>,
      completeRecovery: (token: string, newPassword: string, idempotencyKey: string, signal: AbortSignal) => request('/v1/auth/recovery/complete', { allowEmpty: true, body: { token, newPassword }, idempotencyKey, method: 'POST', signal }, () => undefined) as Promise<void>,
      approveDevice: (userCode: string, signal: AbortSignal) => request('/v1/auth/device/approve', { body: { userCode }, method: 'POST', signal }, value => isRecord(value) && value.approved === true ? true : undefined).then(() => undefined),
      revokeSession: (sessionId: string, signal: AbortSignal) => request(`/v1/auth/sessions/${encodeURIComponent(sessionId)}`, { allowEmpty: true, idempotencyKey: `session-revoke-${sessionId.slice(0, 24)}`, method: 'DELETE', signal }, () => undefined) as Promise<void>,
      logout: (signal: AbortSignal) => request('/v1/auth/logout', { allowEmpty: true, method: 'POST', signal }, () => undefined) as Promise<void>,
    },
    reads: {
      async publicSummary(signal) {
        const health = await request('/v1/health', { signal }, parseHealth);
        const source = health.service ?? (health.contractVersion ? `BugLogin API contract ${health.contractVersion}` : 'BugLogin API');
        return { release: null, status: { label: 'Operational', detail: `${source} reports healthy.` } } satisfies PublicSummary;
      },
      account: signal => request('/v1/account', { signal }, parseAccount),
      organizations,
      workspaces,
      workspace: async (id, signal) => request(workspacePath(scopeFor(id)), { signal }, parseWorkspace),
      workspaceOperational,
      async adminSummary(signal) {
        const [account, organizationRows, workspaceRows] = await Promise.all([request('/v1/account', { signal }, parseAccount), organizations(signal), workspaces(signal)]);
        const operational = await Promise.all(workspaceRows.map(workspace => workspaceOperational(workspace.id, signal)));
        return { account, organizations: organizationRows, workspaces: operational } satisfies AdminSummary;
      },
      profiles: signal => request('/v1/profiles', { signal }, value => parseArray(value, parseVersionedProfile)),
      operation: (operationId, signal) => request(`/v1/operations/${encodeURIComponent(operationId)}`, { signal }, parseOperation),
      proxies: (workspaceId, signal) => request(`${workspaceBase(workspaceId)}/proxies`, { signal }, parseProxyPage),
      proxy: (workspaceId, proxyId, signal) => request(`${workspaceBase(workspaceId)}/proxies/${encodeURIComponent(proxyId)}`, { signal }, parseProxyRecord),
    },
    writes: {
      createOrganization: (input, mutationOptions) => mutation('/v1/organizations', 'POST', input, mutationOptions, parseNull).then(() => undefined),
      updateOrganization: (id, input, mutationOptions) => mutation(`/v1/organizations/${encodeURIComponent(id)}`, 'PATCH', input, mutationOptions, parseOrganization),
      createWorkspace: (organizationId, workspace, mutationOptions) => mutation(`/v1/organizations/${encodeURIComponent(organizationId)}/workspaces`, 'POST', workspace, mutationOptions, parseWorkspace),
      updateWorkspace: async (id, input, mutationOptions) => mutation(workspacePath(scopeFor(id)), 'PATCH', input, mutationOptions, parseWorkspace),
      createTeam: (workspaceId, team, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/teams`, 'POST', team, mutationOptions, parseTeamRecord),
      updateTeam: (workspaceId, teamId, input, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/teams/${encodeURIComponent(teamId)}`, 'PUT', input, mutationOptions, parseTeamRecord),
      deleteTeam: (workspaceId, teamId, input, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/teams/${encodeURIComponent(teamId)}`, 'DELETE', input, mutationOptions, parseTeamRecord),
      inviteMember: (workspaceId, member, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/members`, 'POST', member, mutationOptions, parseMember),
      activateMember: (workspaceId, memberId, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/activate`, 'PUT', undefined, mutationOptions, parseMember),
      suspendMember: (workspaceId, memberId, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/suspend`, 'PUT', undefined, mutationOptions, parseMember),
      updateMemberRole: (workspaceId, memberId, input, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/role`, 'PUT', input, mutationOptions, parseMember),
      createGrant: (workspaceId, input, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/grants`, 'POST', input, mutationOptions, parseGrant),
      revokeGrant: (workspaceId, grantId, mutationOptions) => mutation(`${workspaceBase(workspaceId)}/grants/${encodeURIComponent(grantId)}`, 'DELETE', undefined, mutationOptions, parseGrantRevocationReceipt),
      createProfile: (input: ProfileCreateInput, mutationOptions) => mutation('/v1/profiles', 'POST', input, mutationOptions, parseVersionedProfile),
      launchProfile: (profileId: string, input: LaunchInput, mutationOptions) => mutation(`/v1/profiles/${encodeURIComponent(profileId)}/launch`, 'POST', input, mutationOptions, parseOperation),
      stopProfile: (profileId: string, mutationOptions) => mutation(`/v1/profiles/${encodeURIComponent(profileId)}/stop`, 'POST', undefined, mutationOptions, parseOperation),
      parseProxies: (text: string, signal: AbortSignal) => request('/v1/proxies/parse', { signal, method: 'POST', textBody: text }, parseProxyParseReport),
      createProxy: (input: ProxyCreateInput, mutationOptions) => mutation(`${workspaceBase(input.workspaceId)}/proxies`, 'POST', input, mutationOptions, parseProxyCreateResult),
      updateProxy: (input: ProxyUpdateInput, mutationOptions) => mutation(`${workspaceBase(input.workspaceId)}/proxies/${encodeURIComponent(input.proxyId)}`, 'PATCH', input, mutationOptions, parseProxyRecord),
      deleteProxy: (input: ProxyDeleteInput, mutationOptions) => mutation(`${workspaceBase(input.workspaceId)}/proxies/${encodeURIComponent(input.proxyId)}`, 'DELETE', input, mutationOptions, parseProxyRecord),
      inspectMigration: (input: MigrationInspectInput, mutationOptions) => mutation('/v1/migrations/inspect', 'POST', input, mutationOptions, parseOperation),
      importMigration: (input: MigrationImportInput, mutationOptions) => mutation('/v1/migrations/import', 'POST', input, mutationOptions, parseOperation),
    },
  };

  function mutation<T>(path: string, method: 'DELETE' | 'PATCH' | 'POST' | 'PUT', body: unknown, mutationOptions: MutationOptions, parse: (value: unknown) => T | undefined): Promise<T> {
    return request(path, { body, idempotencyKey: mutationOptions.idempotencyKey, method, signal: mutationOptions.signal }, parse);
  }

  function workspaceBase(id: string): string {
    scopeFor(id);
    return `/v1/workspaces/${encodeURIComponent(id)}`;
  }
}

function workspacePath(scope: WorkspaceScope): string {
  return `/v1/organizations/${encodeURIComponent(scope.organizationId)}/workspaces/${encodeURIComponent(scope.workspaceId)}`;
}

function parseHealth(value: unknown): Health | undefined {
  if (!isRecord(value) || value.status !== 'ok') return undefined;
  if (value.service !== undefined && typeof value.service !== 'string') return undefined;
  if (value.contractVersion !== undefined && typeof value.contractVersion !== 'string') return undefined;
  return { status: 'ok', ...(typeof value.service === 'string' ? { service: value.service } : {}), ...(typeof value.contractVersion === 'string' ? { contractVersion: value.contractVersion } : {}) };
}

function parseAccount(value: unknown): AccountSummary | undefined {
  if (!isRecord(value)) return undefined;
  const accountId = typeof value.accountId === 'string' ? value.accountId : typeof value.id === 'string' ? value.id : undefined;
  if (!accountId) return undefined;
  if (value.revision !== undefined && !isNonNegativeInteger(value.revision)) return undefined;
  return {
    accountId,
    ...(typeof value.status === 'string' ? { status: value.status } : {}),
    ...(typeof value.revision === 'number' ? { revision: value.revision } : {}),
    ...(typeof value.createdAt === 'string' ? { createdAt: value.createdAt } : {}),
    ...(typeof value.updatedAt === 'string' ? { updatedAt: value.updatedAt } : {}),
  };
}

function parseWebSession(value: unknown): WebSession | undefined {
  if (!isRecord(value) || typeof value.accountId !== 'string' || typeof value.expiresAt !== 'number') return undefined;
  return { accountId: value.accountId, expiresAt: value.expiresAt };
}

function parseApiToken(value: unknown): ApiTokenSummary | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.name !== 'string' || !isStringArray(value.scopes) || !isNonNegativeInteger(value.createdAt) || !isNonNegativeInteger(value.expiresAt) || (value.revokedAt !== null && value.revokedAt !== undefined && !isNonNegativeInteger(value.revokedAt))) return undefined;
  return { id: value.id, name: value.name, scopes: value.scopes, createdAt: value.createdAt, expiresAt: value.expiresAt, revokedAt: typeof value.revokedAt === 'number' ? value.revokedAt : null };
}

function parseWebSessionSummary(value: unknown): WebSessionSummary | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || !['browser', 'desktop', 'api'].includes(String(value.kind)) || (typeof value.deviceId !== 'string' && value.deviceId !== null) || (typeof value.createdAt !== 'number' && value.createdAt !== null) || typeof value.expiresAt !== 'number' || (typeof value.revokedAt !== 'number' && value.revokedAt !== null) || typeof value.current !== 'boolean') return undefined;
  return { id: value.id, kind: value.kind as WebSessionSummary['kind'], deviceId: value.deviceId, createdAt: value.createdAt, expiresAt: value.expiresAt, revokedAt: value.revokedAt, current: value.current };
}

function parseOrganization(value: unknown): OrganizationSummary | undefined {
  return isRecord(value) && typeof value.id === 'string' && typeof value.accountId === 'string' && typeof value.name === 'string' && typeof value.createdAt === 'string'
    ? { id: value.id, accountId: value.accountId, name: value.name, createdAt: value.createdAt }
    : undefined;
}

function parseWorkspace(value: unknown): WorkspaceSummary | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.organizationId !== 'string' || typeof value.name !== 'string' || typeof value.createdAt !== 'string' || !isRecord(value.policy) || !isNonNegativeInteger(value.policy.maxProfiles) || !isNonNegativeInteger(value.policy.maxMembers) || !isNonNegativeInteger(value.policy.maxStorageBytes) || typeof value.policy.offlineEntitled !== 'boolean') return undefined;
  return { id: value.id, organizationId: value.organizationId, name: value.name, policy: { maxProfiles: value.policy.maxProfiles, maxMembers: value.policy.maxMembers, maxStorageBytes: value.policy.maxStorageBytes, offlineEntitled: value.policy.offlineEntitled }, createdAt: value.createdAt };
}

function parseTeam(value: unknown): TeamSummary | undefined {
  return isRecord(value) && typeof value.id === 'string' && typeof value.workspaceId === 'string' && typeof value.name === 'string' && isStringArray(value.memberIds)
    ? { id: value.id, workspaceId: value.workspaceId, name: value.name, memberIds: value.memberIds }
    : undefined;
}

function parseTeamRecord(value: unknown): TeamRecord | undefined {
  if (!isRecord(value) || !isNonNegativeInteger(value.revision)) return undefined;
  const team = parseTeam(value.team);
  return team ? { team, revision: value.revision } : undefined;
}

function parseMember(value: unknown): MemberSummary | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.accountId !== 'string' || typeof value.workspaceId !== 'string' || !isMemberRole(value.role) || !isMemberStatus(value.status)) return undefined;
  if (value.teamId !== null && value.teamId !== undefined && typeof value.teamId !== 'string') return undefined;
  if (value.inviteExpiresAt !== null && value.inviteExpiresAt !== undefined && !isNonNegativeInteger(value.inviteExpiresAt)) return undefined;
  return { id: value.id, accountId: value.accountId, workspaceId: value.workspaceId, teamId: typeof value.teamId === 'string' ? value.teamId : null, role: value.role, status: value.status, inviteExpiresAt: typeof value.inviteExpiresAt === 'number' ? value.inviteExpiresAt : null };
}

function parseNull(value: unknown): null | undefined { return value === null ? null : undefined; }

function parseResource(value: unknown): ResourceSummary | undefined {
  return isRecord(value) && typeof value.id === 'string' && typeof value.workspaceId === 'string' && typeof value.kind === 'string' && typeof value.state === 'string' && typeof value.displayName === 'string'
    ? { id: value.id, workspaceId: value.workspaceId, kind: value.kind, state: value.state, displayName: value.displayName }
    : undefined;
}

function parseGrant(value: unknown): GrantSummary | undefined {
  return isRecord(value) && typeof value.id === 'string' && typeof value.workspaceId === 'string' && typeof value.resourceId === 'string' && typeof value.memberId === 'string' && typeof value.action === 'string' && typeof value.effect === 'string'
    ? { id: value.id, workspaceId: value.workspaceId, resourceId: value.resourceId, memberId: value.memberId, action: value.action, effect: value.effect }
    : undefined;
}

function parseGrantRevocationReceipt(value: unknown): GrantRevocationReceipt | undefined {
  return isRecord(value) && typeof value.grantId === 'string' && value.revoked === true ? { grantId: value.grantId, revoked: true } : undefined;
}

function parseProxyPage(value: unknown): ProxyPage | undefined {
  return isRecord(value) && Array.isArray(value.items) ? (() => {
    const items = value.items.map(parseProxyRecord);
    return items.every((item): item is ProxyRecord => item !== undefined) ? { items } : undefined;
  })() : undefined;
}

function parseProxyRecord(value: unknown): ProxyRecord | undefined {
  if (!isRecord(value) || typeof value.workspaceId !== 'string' || !isPositiveInteger(value.revision)) return undefined;
  const proxy = parseProxyView(value.proxy);
  return proxy ? { workspaceId: value.workspaceId, proxy, revision: value.revision } : undefined;
}

function parseProxyView(value: unknown): ProxyView | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || !isProxyProtocol(value.protocol) || typeof value.host !== 'string' || !isPositiveInteger(value.port) || value.port > 65535 || typeof value.hasCredential !== 'boolean' || typeof value.rotationConfigured !== 'boolean' || !isStringArray(value.bypassRules) || typeof value.createdAt !== 'string' || typeof value.updatedAt !== 'string' || (value.name !== undefined && typeof value.name !== 'string')) return undefined;
  return { id: value.id, ...(typeof value.name === 'string' ? { name: value.name } : {}), protocol: value.protocol, host: value.host, port: value.port, hasCredential: value.hasCredential, rotationConfigured: value.rotationConfigured, bypassRules: value.bypassRules, createdAt: value.createdAt, updatedAt: value.updatedAt };
}

function parseProxyCreateResult(value: unknown): ProxyCreateResult | undefined {
  if (!isRecord(value) || typeof value.deduplicated !== 'boolean') return undefined;
  const record = parseProxyRecord(value.record);
  return record ? { record, deduplicated: value.deduplicated } : undefined;
}

function parseVersionedProfile(value: unknown): VersionedProfile | undefined {
  if (!isRecord(value) || !isNonNegativeInteger(value.revision)) return undefined;
  const profile = parseProfile(value.value);
  return profile ? { revision: value.revision, value: profile } : undefined;
}

function parseProfile(value: unknown): Profile | undefined {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.name !== 'string' || !isRuntime(value.runtime) || !isProfileState(value.state) || typeof value.archived !== 'boolean' || typeof value.createdAt !== 'string' || typeof value.updatedAt !== 'string' || !isStringArray(value.groupIds) || !isStringArray(value.tags)) return undefined;
  if (!optionalStringOrNull(value.runtimeVersion) || !optionalStringOrNull(value.note) || !optionalStringOrNull(value.proxyId) || !optionalStringOrNull(value.vpnId) || !optionalStringOrNull(value.extensionGroupId) || !optionalEnumOrNull(value.syncMode, isSyncMode)) return undefined;
  return {
    id: value.id,
    name: value.name,
    runtime: value.runtime,
    runtimeVersion: typeof value.runtimeVersion === 'string' ? value.runtimeVersion : null,
    runtimeConfig: value.runtimeConfig === undefined ? null : value.runtimeConfig,
    state: value.state,
    archived: value.archived,
    fingerprint: value.fingerprint === undefined ? null : value.fingerprint,
    note: typeof value.note === 'string' ? value.note : null,
    proxyId: typeof value.proxyId === 'string' ? value.proxyId : null,
    vpnId: typeof value.vpnId === 'string' ? value.vpnId : null,
    groupIds: value.groupIds,
    tags: value.tags,
    extensionGroupId: typeof value.extensionGroupId === 'string' ? value.extensionGroupId : null,
    syncMode: isSyncMode(value.syncMode) ? value.syncMode : null,
    createdAt: value.createdAt,
    updatedAt: value.updatedAt,
  };
}

function parseOperation(value: unknown): Operation | undefined {
  if (!isRecord(value) || typeof value.operationId !== 'string' || typeof value.commandId !== 'string' || !isOperationStatus(value.status) || (value.progress !== null && value.progress !== undefined && typeof value.progress !== 'number')) return undefined;
  return { operationId: value.operationId, commandId: value.commandId, scope: value.scope, status: value.status, progress: typeof value.progress === 'number' ? value.progress : null, ...(value.result === undefined ? {} : { result: value.result }), ...(isRecord(value.error) ? { error: value.error as Problem } : {}) };
}

function parseProxyParseReport(value: unknown): ProxyParseReport | undefined {
  if (!isRecord(value) || !isNonNegativeInteger(value.validCount) || !isNonNegativeInteger(value.invalidCount) || !isNonNegativeInteger(value.duplicateCount)) return undefined;
  const rows = Array.isArray(value.rows) ? value.rows.map(parseProxyParseRow) : undefined;
  return rows && rows.every((row): row is ProxyParseReport['rows'][number] => row !== undefined) ? { validCount: value.validCount, invalidCount: value.invalidCount, duplicateCount: value.duplicateCount, rows } : undefined;
}

function parseProxyParseRow(value: unknown): ProxyParseReport['rows'][number] | undefined {
  if (!isRecord(value) || typeof value.valid !== 'boolean' || !optionalStringOrNull(value.protocol) || !optionalStringOrNull(value.endpoint)) return undefined;
  if (value.line !== undefined && !isNonNegativeInteger(value.line)) return undefined;
  if (value.error !== undefined && typeof value.error !== 'string') return undefined;
  return { protocol: typeof value.protocol === 'string' ? value.protocol : null, endpoint: typeof value.endpoint === 'string' ? value.endpoint : null, valid: value.valid, ...(typeof value.line === 'number' ? { line: value.line } : {}), ...(typeof value.error === 'string' ? { error: value.error } : {}) };
}

function parseArray<T>(value: unknown, parse: (item: unknown) => T | undefined): T[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const items = value.map(parse);
  return items.every((item): item is T => item !== undefined) ? items : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> { return !!value && typeof value === 'object' && !Array.isArray(value); }
function isNonNegativeInteger(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0; }
function isPositiveInteger(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 1; }
function isStringArray(value: unknown): value is string[] { return Array.isArray(value) && value.every(item => typeof item === 'string'); }
function optionalStringOrNull(value: unknown): boolean { return value === undefined || value === null || typeof value === 'string'; }
function optionalEnumOrNull<T>(value: unknown, guard: (value: unknown) => value is T): boolean { return value === undefined || value === null || guard(value); }
function isRuntime(value: unknown): value is Runtime { return value === 'wayfern' || value === 'bugox' || value === 'bugium' || value === 'bugmini' || value === 'bugthor' || value === 'bughelium'; }
function isProfileState(value: unknown): value is ProfileState { return value === 'created' || value === 'ready' || value === 'launching' || value === 'running' || value === 'stopping' || value === 'failed' || value === 'recovering'; }
function isSyncMode(value: unknown): value is SyncMode { return value === 'local' || value === 'managed' || value === 'disabled'; }
function isProxyProtocol(value: unknown): value is import('./api.ts').ProxyProtocol { return value === 'http' || value === 'https' || value === 'socks4' || value === 'socks5'; }
function isOperationStatus(value: unknown): value is OperationStatus { return value === 'queued' || value === 'running' || value === 'completed' || value === 'failed' || value === 'cancelled'; }
function isMemberRole(value: unknown): value is MemberSummary['role'] { return value === 'owner' || value === 'admin' || value === 'operator' || value === 'member'; }
function isMemberStatus(value: unknown): value is MemberSummary['status'] { return value === 'invited' || value === 'active' || value === 'suspended'; }

function validIdempotencyKey(value: string, correlationId: string): string {
  if (/^[\x20-\x7E]{16,128}$/.test(value)) return value;
  throw new WebApiError(problem('idempotency_key_invalid', 'Valid idempotency key required', 400, correlationId));
}

async function responseProblem(response: Response, correlationId: string): Promise<Problem> {
  try { const parsed = parseProblem(await response.json()); if (parsed) return parsed; } catch {}
  return problem('http_error', response.statusText || 'Request failed', response.status, correlationId);
}

function parseProblem(value: unknown): Problem | undefined {
  if (!isRecord(value) || typeof value.type !== 'string' || typeof value.title !== 'string' || typeof value.status !== 'number' || typeof value.code !== 'string' || typeof value.correlationId !== 'string') return undefined;
  return { type: value.type, title: value.title, status: value.status, code: value.code, ...(typeof value.detail === 'string' ? { detail: value.detail } : {}), ...(typeof value.instance === 'string' ? { instance: value.instance } : {}), correlationId: value.correlationId, retryable: value.retryable === true, fields: isRecord(value.fields) ? value.fields : {} };
}

function problem(code: string, title: string, status: number, correlationId: string): Problem {
  return { type: `https://buglogin.local/problems/${code}`, title, status, code, correlationId, retryable: false, fields: {} };
}

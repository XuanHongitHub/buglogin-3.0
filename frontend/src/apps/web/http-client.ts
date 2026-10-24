import {
  ACTION_VALUES,
  EFFECT_VALUES,
  MEMBER_ROLE_VALUES,
  MEMBER_STATUS_VALUES,
  PROFILE_STATE_VALUES,
  PROXY_PROTOCOL_VALUES,
  RESOURCE_KIND_VALUES,
  RESOURCE_STATE_VALUES,
  RUNTIME_VALUES,
  SYNC_MODE_VALUES,
} from "../../generated/command-registry.ts";
import {
  type AccountSummary,
  type AdminSummary,
  type ApiTokenCreateInput,
  type ApiTokenSummary,
  type AuditEventPage,
  type GrantRevocationReceipt,
  type GrantSummary,
  type Group,
  type GroupCreateInput,
  type GroupDeleteInput,
  type GroupRecord,
  type GroupUpdateInput,
  type JsonValue,
  type LaunchInput,
  type MemberSummary,
  type MigrationImportInput,
  type MigrationInspectInput,
  type MutationOptions,
  type Operation,
  type OperationStatus,
  type OrganizationRecord,
  type OrganizationSummary,
  type Problem,
  type Profile,
  type ProfileCreateInput,
  type ProfileGroupsReplaceInput,
  type ProfileProxyAssignment,
  type ProfileProxyAssignmentGetInput,
  type ProfileProxyAssignmentSetInput,
  type ProfileRecord,
  type ProfileState,
  type ProxyCreateInput,
  type ProxyCreateResult,
  type ProxyDeleteInput,
  type ProxyPage,
  type ProxyParseReport,
  type ProxyRecord,
  type ProxyUpdateInput,
  type ProxyView,
  type PublicSummary,
  type ResourceScope,
  type ResourceSummary,
  type Runtime,
  type SyncMode,
  type TeamRecord,
  type TeamSummary,
  type VersionedProfile,
  type VersionedProfilePage,
  type WebApiClient,
  WebApiError,
  type WebSession,
  type WebSessionSummary,
  type WorkspaceOperationalData,
  type WorkspaceSummary,
} from "./api.ts";

export type WorkspaceScope = { organizationId: string; workspaceId: string };
export type WebApiClientOptions = {
  baseUrl: string;
  accessToken?: string;
  workspaceScopes?: readonly WorkspaceScope[];
  fetch?: typeof fetch;
  correlationId?: () => string;
};

type Readiness = { status: "ready"; service: string };
type RequestOptions = {
  allowEmpty?: boolean;
  body?: unknown;
  textBody?: string;
  idempotencyKey?: string;
  method?: "DELETE" | "GET" | "PATCH" | "POST" | "PUT";
  signal: AbortSignal;
};

export function createWebApiClient(options: WebApiClientOptions): WebApiClient {
  const fetchRequest = options.fetch ?? globalThis.fetch;
  const baseUrl = options.baseUrl.replace(/\/$/, "");
  const nextCorrelationId =
    options.correlationId ?? (() => `cor_${globalThis.crypto.randomUUID()}`);
  const workspaceScopes = options.workspaceScopes ?? [];

  async function request<T>(
    path: string,
    requestOptions: RequestOptions,
    parse: (value: unknown) => T | undefined,
  ): Promise<T> {
    const correlationId = nextCorrelationId();
    const headers = new Headers({
      Accept: "application/json, application/problem+json",
      "X-Correlation-Id": correlationId,
    });
    if (options.accessToken)
      headers.set("Authorization", `Bearer ${options.accessToken}`);
    const method = requestOptions.method ?? "GET";
    if (requestOptions.idempotencyKey)
      headers.set(
        "Idempotency-Key",
        validIdempotencyKey(requestOptions.idempotencyKey, correlationId),
      );
    if (requestOptions.body !== undefined)
      headers.set("Content-Type", "application/json");
    if (requestOptions.textBody !== undefined)
      headers.set("Content-Type", "text/plain; charset=utf-8");
    if (
      method !== "GET" &&
      !options.accessToken &&
      typeof document !== "undefined"
    ) {
      const csrf = document.cookie
        .split(";")
        .map((value) => value.trim())
        .find((value) => value.startsWith("__Host-buglogin_csrf="))
        ?.split("=")
        .slice(1)
        .join("=");
      if (csrf) headers.set("X-CSRF-Token", csrf);
    }
    const response = await fetchRequest(`${baseUrl}${path}`, {
      credentials: "include",
      headers,
      method,
      signal: requestOptions.signal,
      ...(requestOptions.textBody !== undefined
        ? { body: requestOptions.textBody }
        : requestOptions.body === undefined
          ? {}
          : { body: JSON.stringify(requestOptions.body) }),
    });
    if (!response.ok)
      throw new WebApiError(await responseProblem(response, correlationId));
    if (requestOptions.allowEmpty) return undefined as T;
    try {
      const parsed = parse(await response.json());
      if (parsed !== undefined) return parsed;
    } catch {}
    throw new WebApiError(
      problem("invalid_response", "Invalid API response", 502, correlationId),
    );
  }

  function scopeFor(id: string): WorkspaceScope {
    const matches = workspaceScopes.filter((scope) => scope.workspaceId === id);
    if (matches.length !== 1)
      throw new WebApiError(
        problem(
          "workspace_scope_missing",
          "Workspace organization scope is missing",
          400,
          nextCorrelationId(),
        ),
      );
    return matches[0];
  }

  async function organizations(
    signal: AbortSignal,
  ): Promise<OrganizationSummary[]> {
    return request("/v1/organizations", { signal }, (value) =>
      parseArray(value, parseOrganization),
    );
  }

  async function workspaces(signal: AbortSignal): Promise<WorkspaceSummary[]> {
    const organizationIds = [
      ...new Set(workspaceScopes.map((scope) => scope.organizationId)),
    ];
    if (organizationIds.length === 0)
      throw new WebApiError(
        problem(
          "workspace_scope_missing",
          "Workspace organization scope is missing",
          400,
          nextCorrelationId(),
        ),
      );
    const pages = await Promise.all(
      organizationIds.map((id) =>
        request(
          `/v1/organizations/${encodeURIComponent(id)}/workspaces`,
          { signal },
          (value) => parseArray(value, parseWorkspace),
        ),
      ),
    );
    const allowed = new Set(workspaceScopes.map((scope) => scope.workspaceId));
    return pages.flat().filter((workspace) => allowed.has(workspace.id));
  }

  async function workspaceOperational(
    id: string,
    signal: AbortSignal,
  ): Promise<WorkspaceOperationalData> {
    const scope = scopeFor(id);
    const workspaceBase = `/v1/workspaces/${encodeURIComponent(id)}`;
    const [workspace, teams, members, resources, grants] = await Promise.all([
      request(workspacePath(scope), { signal }, parseWorkspace),
      request(`${workspaceBase}/teams`, { signal }, (value) =>
        parseArray(value, (item) => parseTeamRecord(item)?.team),
      ),
      fetchAllPages(`${workspaceBase}/members/page`, signal, parseMember),
      fetchAllPages(`${workspaceBase}/resources/page`, signal, parseResource),
      request(`${workspaceBase}/grants`, { signal }, (value) =>
        parseArray(value, parseGrant),
      ),
    ]);
    return { workspace, teams, members, resources, grants };
  }

  async function fetchAllPages<T>(
    path: string,
    signal: AbortSignal,
    parseItem: (value: unknown) => T | undefined,
  ): Promise<T[]> {
    const items: T[] = [];
    const seenCursors = new Set<string>();
    let cursor: string | null = null;
    do {
      const query = new URLSearchParams({ limit: "100" });
      if (cursor !== null) query.set("cursor", cursor);
      const page = await request(
        `${path}?${query.toString()}`,
        { signal },
        (value) => {
          if (!isRecord(value) || !Array.isArray(value.items)) return undefined;
          const parsedItems = parseArray(value.items, parseItem);
          const nextCursor = value.nextCursor;
          if (!parsedItems) return undefined;
          if (nextCursor === null)
            return { items: parsedItems, nextCursor: null };
          if (typeof nextCursor === "string" && nextCursor.length > 0)
            return { items: parsedItems, nextCursor };
          return undefined;
        },
      );
      items.push(...page.items);
      cursor = page.nextCursor;
      if (cursor !== null) {
        if (cursor.length > 512 || seenCursors.has(cursor))
          throw new WebApiError({
            type: "about:blank",
            title: "Invalid page cursor",
            status: 502,
            code: "api_page_cursor_invalid",
            retryable: false,
            correlationId: nextCorrelationId(),
            fields: {},
          });
        seenCursors.add(cursor);
      }
    } while (cursor !== null);
    return items;
  }

  return {
    auth: {
      login: (identifier: string, password: string, signal: AbortSignal) =>
        request(
          "/v1/auth/login",
          { body: { identifier, password }, method: "POST", signal },
          parseWebSession,
        ),
      session: (signal: AbortSignal) =>
        request("/v1/auth/session", { signal }, parseWebSession),
      sessions: (signal: AbortSignal) =>
        request("/v1/auth/sessions", { signal }, (value) =>
          isRecord(value) && Array.isArray(value.sessions)
            ? value.sessions
                .map(parseWebSessionSummary)
                .filter((item): item is WebSessionSummary => item !== undefined)
            : undefined,
        ),
      apiTokens: (signal) =>
        request("/v1/auth/api-tokens", { signal }, (value) =>
          isRecord(value) && Array.isArray(value.tokens)
            ? value.tokens
                .map(parseApiToken)
                .filter((item): item is ApiTokenSummary => item !== undefined)
            : undefined,
        ),
      createApiToken: (input: ApiTokenCreateInput, signal: AbortSignal) =>
        request(
          "/v1/auth/api-tokens",
          { body: input, method: "POST", signal },
          (value) => {
            if (!isRecord(value) || typeof value.token !== "string") {
              return undefined;
            }
            const tokenRecord = parseApiToken(value.tokenRecord);
            return tokenRecord
              ? { token: value.token, tokenRecord }
              : undefined;
          },
        ),
      revokeApiToken: (tokenId: string, signal: AbortSignal) =>
        request(
          `/v1/auth/api-tokens/${encodeURIComponent(tokenId)}`,
          {
            allowEmpty: true,
            idempotencyKey: `api-token-revoke-${tokenId.slice(0, 24)}`,
            method: "DELETE",
            signal,
          },
          () => undefined,
        ) as Promise<void>,
      requestRecovery: (identifier: string, signal: AbortSignal) =>
        request(
          "/v1/auth/recovery/request",
          { allowEmpty: true, body: { identifier }, method: "POST", signal },
          () => undefined,
        ) as Promise<void>,
      completeRecovery: (
        token: string,
        newPassword: string,
        idempotencyKey: string,
        signal: AbortSignal,
      ) =>
        request(
          "/v1/auth/recovery/complete",
          {
            allowEmpty: true,
            body: { token, newPassword },
            idempotencyKey,
            method: "POST",
            signal,
          },
          () => undefined,
        ) as Promise<void>,
      approveDevice: (userCode: string, signal: AbortSignal) =>
        request(
          "/v1/auth/device/approve",
          { body: { userCode }, method: "POST", signal },
          (value) =>
            isRecord(value) && value.approved === true ? true : undefined,
        ).then(() => undefined),
      revokeSession: (sessionId: string, signal: AbortSignal) =>
        request(
          `/v1/auth/sessions/${encodeURIComponent(sessionId)}`,
          {
            allowEmpty: true,
            idempotencyKey: `session-revoke-${sessionId.slice(0, 24)}`,
            method: "DELETE",
            signal,
          },
          () => undefined,
        ) as Promise<void>,
      logout: (signal: AbortSignal) =>
        request(
          "/v1/auth/logout",
          {
            allowEmpty: true,
            idempotencyKey: `auth-logout-${globalThis.crypto.randomUUID()}`,
            method: "POST",
            signal,
          },
          () => undefined,
        ) as Promise<void>,
    },
    reads: {
      async publicSummary(signal) {
        const readiness = await request(
          "/v1/readyz",
          { signal },
          parseReadiness,
        );
        return {
          release: null,
          status: {
            label: "Operational",
            detail: `${readiness.service} reports ready.`,
          },
        } satisfies PublicSummary;
      },
      account: (signal) => request("/v1/account", { signal }, parseAccount),
      organizations,
      workspaces,
      workspace: async (id, signal) =>
        request(workspacePath(scopeFor(id)), { signal }, parseWorkspace),
      workspaceOperational,
      async adminSummary(signal) {
        const [account, organizationRows, workspaceRows] = await Promise.all([
          request("/v1/account", { signal }, parseAccount),
          organizations(signal),
          workspaces(signal),
        ]);
        const operational = await Promise.all(
          workspaceRows.map((workspace) =>
            workspaceOperational(workspace.id, signal),
          ),
        );
        return {
          account,
          organizations: organizationRows,
          workspaces: operational,
        } satisfies AdminSummary;
      },
      profiles: (cursor, limit, signal) => {
        if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
          throw new WebApiError(
            problem(
              "invalid_page_limit",
              "Profile page limit must be between 1 and 100",
              400,
              nextCorrelationId(),
            ),
          );
        }
        const query = new URLSearchParams({ limit: String(limit) });
        if (cursor !== null) query.set("cursor", cursor);
        return request(
          `/v1/profiles?${query}`,
          { signal },
          parseVersionedProfilePage,
        );
      },
      operation: (operationId, signal) =>
        request(
          `/v1/operations/${encodeURIComponent(operationId)}`,
          { signal },
          parseOperation,
        ),
      proxies: (workspaceId, signal) =>
        request(
          `${workspaceBase(workspaceId)}/proxies`,
          { signal },
          parseProxyPage,
        ),
      proxy: (workspaceId, proxyId, signal) =>
        request(
          `${workspaceBase(workspaceId)}/proxies/${encodeURIComponent(proxyId)}`,
          { signal },
          parseProxyRecord,
        ),
      profileProxyAssignment: (input: ProfileProxyAssignmentGetInput, signal) =>
        request(
          `${workspaceBase(input.workspaceId)}/profiles/${encodeURIComponent(input.profileId)}/proxy-assignment`,
          { signal },
          parseProfileProxyAssignment,
        ),
      groups: (workspaceId, signal) =>
        request(`${workspaceBase(workspaceId)}/groups`, { signal }, (value) =>
          parseArray(value, parseGroupRecord),
        ),
      group: (workspaceId, groupId, signal) =>
        request(
          `${workspaceBase(workspaceId)}/groups/${encodeURIComponent(groupId)}`,
          { signal },
          parseGroupRecord,
        ),
      auditEvents: (workspaceId, cursor, limit, signal) => {
        const query = new URLSearchParams({ limit: String(limit) });
        if (cursor !== null) query.set("cursor", cursor);
        return request(
          `${workspaceBase(workspaceId)}/audit-events?${query}`,
          { signal },
          parseAuditEventPage,
        );
      },
    },
    writes: {
      createOrganization: (input, mutationOptions) =>
        mutation(
          "/v1/organizations",
          "POST",
          input,
          mutationOptions,
          parseOrganizationRecord,
        ),
      updateOrganization: (id, input, mutationOptions) =>
        mutation(
          `/v1/organizations/${encodeURIComponent(id)}`,
          "PATCH",
          input,
          mutationOptions,
          parseOrganization,
        ),
      createWorkspace: (organizationId, workspace, mutationOptions) =>
        mutation(
          `/v1/organizations/${encodeURIComponent(organizationId)}/workspaces`,
          "POST",
          workspace,
          mutationOptions,
          parseWorkspace,
        ),
      updateWorkspace: async (id, input, mutationOptions) =>
        mutation(
          workspacePath(scopeFor(id)),
          "PATCH",
          input,
          mutationOptions,
          parseWorkspace,
        ),
      createTeam: (workspaceId, team, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/teams`,
          "POST",
          team,
          mutationOptions,
          parseTeamRecord,
        ),
      updateTeam: (workspaceId, teamId, input, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/teams/${encodeURIComponent(teamId)}`,
          "PUT",
          input,
          mutationOptions,
          parseTeamRecord,
        ),
      deleteTeam: (workspaceId, teamId, input, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/teams/${encodeURIComponent(teamId)}`,
          "DELETE",
          input,
          mutationOptions,
          parseTeamRecord,
        ),
      inviteMember: (workspaceId, member, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/members`,
          "POST",
          member,
          mutationOptions,
          parseMember,
        ),
      activateMember: (workspaceId, memberId, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/activate`,
          "PUT",
          undefined,
          mutationOptions,
          parseMember,
        ),
      suspendMember: (workspaceId, memberId, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/suspend`,
          "PUT",
          undefined,
          mutationOptions,
          parseMember,
        ),
      updateMemberRole: (workspaceId, memberId, input, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/members/${encodeURIComponent(memberId)}/role`,
          "PUT",
          input,
          mutationOptions,
          parseMember,
        ),
      createGrant: (workspaceId, input, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/grants`,
          "POST",
          input,
          mutationOptions,
          parseGrant,
        ),
      revokeGrant: (workspaceId, grantId, mutationOptions) =>
        mutation(
          `${workspaceBase(workspaceId)}/grants/${encodeURIComponent(grantId)}`,
          "DELETE",
          undefined,
          mutationOptions,
          parseGrantRevocationReceipt,
        ),
      createProfile: (input: ProfileCreateInput, mutationOptions) =>
        mutation(
          "/v1/profiles",
          "POST",
          input,
          mutationOptions,
          parseVersionedProfile,
        ),
      launchProfile: (profileId: string, input: LaunchInput, mutationOptions) =>
        mutation(
          `/v1/profiles/${encodeURIComponent(profileId)}/launch`,
          "POST",
          input,
          mutationOptions,
          parseOperation,
        ),
      stopProfile: (profileId: string, mutationOptions) =>
        mutation(
          `/v1/profiles/${encodeURIComponent(profileId)}/stop`,
          "POST",
          undefined,
          mutationOptions,
          parseOperation,
        ),
      parseProxies: (text: string, signal: AbortSignal) =>
        request(
          "/v1/proxies/parse",
          { signal, method: "POST", textBody: text },
          parseProxyParseReport,
        ),
      createProxy: (input: ProxyCreateInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/proxies`,
          "POST",
          input,
          mutationOptions,
          parseProxyCreateResult,
        ),
      updateProxy: (input: ProxyUpdateInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/proxies/${encodeURIComponent(input.proxyId)}`,
          "PATCH",
          input,
          mutationOptions,
          parseProxyRecord,
        ),
      deleteProxy: (input: ProxyDeleteInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/proxies/${encodeURIComponent(input.proxyId)}`,
          "DELETE",
          input,
          mutationOptions,
          parseProxyRecord,
        ),
      stageProfileProxyAssignment: (
        input: ProfileProxyAssignmentSetInput,
        mutationOptions,
      ) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/profiles/${encodeURIComponent(input.profileId)}/proxy-assignment`,
          "PUT",
          input,
          mutationOptions,
          parseProfileProxyAssignment,
        ),
      createGroup: (input: GroupCreateInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/groups`,
          "POST",
          input,
          mutationOptions,
          parseGroupRecord,
        ),
      updateGroup: (input: GroupUpdateInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/groups/${encodeURIComponent(input.groupId)}`,
          "PUT",
          input,
          mutationOptions,
          parseGroupRecord,
        ),
      deleteGroup: (input: GroupDeleteInput, mutationOptions) =>
        mutation(
          `${workspaceBase(input.workspaceId)}/groups/${encodeURIComponent(input.groupId)}`,
          "DELETE",
          input,
          mutationOptions,
          parseGroupRecord,
        ),
      replaceProfileGroups: (
        input: ProfileGroupsReplaceInput,
        mutationOptions,
      ) => {
        workspaceBase(input.workspaceId);
        return mutation(
          `/v1/profiles/${encodeURIComponent(input.profileId)}/groups`,
          "PUT",
          input,
          mutationOptions,
          parseProfileRecord,
        );
      },
      inspectMigration: (input: MigrationInspectInput, mutationOptions) =>
        mutation(
          "/v1/migrations/inspect",
          "POST",
          input,
          mutationOptions,
          parseOperation,
        ),
      importMigration: (input: MigrationImportInput, mutationOptions) =>
        mutation(
          "/v1/migrations/import",
          "POST",
          input,
          mutationOptions,
          parseOperation,
        ),
    },
  };

  function mutation<T>(
    path: string,
    method: "DELETE" | "PATCH" | "POST" | "PUT",
    body: unknown,
    mutationOptions: MutationOptions,
    parse: (value: unknown) => T | undefined,
  ): Promise<T> {
    return request(
      path,
      {
        body,
        idempotencyKey: mutationOptions.idempotencyKey,
        method,
        signal: mutationOptions.signal,
      },
      parse,
    );
  }

  function workspaceBase(id: string): string {
    scopeFor(id);
    return `/v1/workspaces/${encodeURIComponent(id)}`;
  }
}

function workspacePath(scope: WorkspaceScope): string {
  return `/v1/organizations/${encodeURIComponent(scope.organizationId)}/workspaces/${encodeURIComponent(scope.workspaceId)}`;
}

function parseReadiness(value: unknown): Readiness | undefined {
  if (
    !isRecord(value) ||
    value.status !== "ready" ||
    typeof value.service !== "string" ||
    value.service.trim().length === 0
  ) {
    return undefined;
  }
  return { status: "ready", service: value.service };
}

function parseAccount(value: unknown): AccountSummary | undefined {
  if (!isRecord(value)) return undefined;
  const accountId =
    typeof value.accountId === "string"
      ? value.accountId
      : typeof value.id === "string"
        ? value.id
        : undefined;
  if (!accountId) return undefined;
  const status = value.status;
  if (
    status !== undefined &&
    status !== "active" &&
    status !== "suspended" &&
    status !== "disabled"
  )
    return undefined;
  if (value.revision !== undefined && !isPositiveInteger(value.revision))
    return undefined;
  return {
    accountId,
    platformAdmin: value.platformAdmin === true,
    managedWorkspaceIds: Array.isArray(value.managedWorkspaceIds)
      ? value.managedWorkspaceIds.filter(
          (workspaceId): workspaceId is string => typeof workspaceId === "string",
        )
      : [],
    ...(status !== undefined ? { status } : {}),
    ...(typeof value.revision === "number" ? { revision: value.revision } : {}),
    ...(typeof value.createdAt === "string"
      ? { createdAt: value.createdAt }
      : {}),
    ...(typeof value.updatedAt === "string"
      ? { updatedAt: value.updatedAt }
      : {}),
  };
}

function parseWebSession(value: unknown): WebSession | undefined {
  if (
    !isRecord(value) ||
    typeof value.accountId !== "string" ||
    typeof value.expiresAt !== "number"
  )
    return undefined;
  return { accountId: value.accountId, expiresAt: value.expiresAt };
}

function parseApiToken(value: unknown): ApiTokenSummary | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    typeof value.name !== "string" ||
    !isStringArray(value.scopes) ||
    !isNonNegativeInteger(value.createdAt) ||
    !isNonNegativeInteger(value.expiresAt) ||
    (value.revokedAt !== null &&
      value.revokedAt !== undefined &&
      !isNonNegativeInteger(value.revokedAt))
  )
    return undefined;
  return {
    id: value.id,
    name: value.name,
    scopes: value.scopes,
    createdAt: value.createdAt,
    expiresAt: value.expiresAt,
    revokedAt: typeof value.revokedAt === "number" ? value.revokedAt : null,
  };
}

function parseWebSessionSummary(value: unknown): WebSessionSummary | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    !["browser", "desktop", "api"].includes(String(value.kind)) ||
    (typeof value.deviceId !== "string" && value.deviceId !== null) ||
    (typeof value.createdAt !== "number" && value.createdAt !== null) ||
    typeof value.expiresAt !== "number" ||
    (typeof value.revokedAt !== "number" && value.revokedAt !== null) ||
    typeof value.current !== "boolean"
  )
    return undefined;
  return {
    id: value.id,
    kind: value.kind as WebSessionSummary["kind"],
    deviceId: value.deviceId,
    createdAt: value.createdAt,
    expiresAt: value.expiresAt,
    revokedAt: value.revokedAt,
    current: value.current,
  };
}

function parseOrganization(value: unknown): OrganizationSummary | undefined {
  return isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.accountId === "string" &&
    typeof value.name === "string" &&
    typeof value.createdAt === "string"
    ? {
        id: value.id,
        accountId: value.accountId,
        name: value.name,
        createdAt: value.createdAt,
      }
    : undefined;
}

function parseOrganizationRecord(
  value: unknown,
): OrganizationRecord | undefined {
  if (!isRecord(value) || !isPositiveInteger(value.revision)) return undefined;
  const organization = parseOrganization(value.organization);
  if (
    !organization ||
    !isRecord(value.policy) ||
    !isNonNegativeInteger(value.policy.maxWorkspaces) ||
    typeof value.policy.offlineEntitled !== "boolean"
  )
    return undefined;
  return {
    organization,
    policy: {
      maxWorkspaces: value.policy.maxWorkspaces,
      offlineEntitled: value.policy.offlineEntitled,
    },
    revision: value.revision,
  };
}

function parseWorkspace(value: unknown): WorkspaceSummary | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    typeof value.organizationId !== "string" ||
    typeof value.name !== "string" ||
    typeof value.createdAt !== "string" ||
    !isRecord(value.policy) ||
    !isNonNegativeInteger(value.policy.maxProfiles) ||
    !isNonNegativeInteger(value.policy.maxMembers) ||
    !isNonNegativeInteger(value.policy.maxStorageBytes) ||
    typeof value.policy.offlineEntitled !== "boolean"
  )
    return undefined;
  return {
    id: value.id,
    organizationId: value.organizationId,
    name: value.name,
    policy: {
      maxProfiles: value.policy.maxProfiles,
      maxMembers: value.policy.maxMembers,
      maxStorageBytes: value.policy.maxStorageBytes,
      offlineEntitled: value.policy.offlineEntitled,
    },
    createdAt: value.createdAt,
  };
}

function parseTeam(value: unknown): TeamSummary | undefined {
  return isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.workspaceId === "string" &&
    typeof value.name === "string" &&
    isStringArray(value.memberIds)
    ? {
        id: value.id,
        workspaceId: value.workspaceId,
        name: value.name,
        memberIds: value.memberIds,
      }
    : undefined;
}

function parseTeamRecord(value: unknown): TeamRecord | undefined {
  if (!isRecord(value) || !isNonNegativeInteger(value.revision))
    return undefined;
  const team = parseTeam(value.team);
  return team ? { team, revision: value.revision } : undefined;
}

function parseMember(value: unknown): MemberSummary | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    typeof value.accountId !== "string" ||
    typeof value.workspaceId !== "string" ||
    !isMemberRole(value.role) ||
    !isMemberStatus(value.status)
  )
    return undefined;
  if (
    value.teamId !== null &&
    value.teamId !== undefined &&
    typeof value.teamId !== "string"
  )
    return undefined;
  if (
    value.inviteExpiresAt !== null &&
    !isNonNegativeInteger(value.inviteExpiresAt)
  )
    return undefined;
  return {
    id: value.id,
    accountId: value.accountId,
    workspaceId: value.workspaceId,
    teamId: typeof value.teamId === "string" ? value.teamId : null,
    role: value.role,
    status: value.status,
    inviteExpiresAt:
      typeof value.inviteExpiresAt === "number" ? value.inviteExpiresAt : null,
  };
}

function parseResource(value: unknown): ResourceSummary | undefined {
  return isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.workspaceId === "string" &&
    isResourceKind(value.kind) &&
    isResourceState(value.state) &&
    typeof value.displayName === "string"
    ? {
        id: value.id,
        workspaceId: value.workspaceId,
        kind: value.kind,
        state: value.state,
        displayName: value.displayName,
      }
    : undefined;
}

function parseGrant(value: unknown): GrantSummary | undefined {
  return isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.workspaceId === "string" &&
    typeof value.resourceId === "string" &&
    typeof value.memberId === "string" &&
    isGrantAction(value.action) &&
    isGrantEffect(value.effect)
    ? {
        id: value.id,
        workspaceId: value.workspaceId,
        resourceId: value.resourceId,
        memberId: value.memberId,
        action: value.action,
        effect: value.effect,
      }
    : undefined;
}

function parseGrantRevocationReceipt(
  value: unknown,
): GrantRevocationReceipt | undefined {
  return isRecord(value) &&
    typeof value.grantId === "string" &&
    value.revoked === true
    ? { grantId: value.grantId, revoked: true }
    : undefined;
}

function parseProxyPage(value: unknown): ProxyPage | undefined {
  return isRecord(value) && Array.isArray(value.items)
    ? (() => {
        const items = value.items.map(parseProxyRecord);
        return items.every((item): item is ProxyRecord => item !== undefined)
          ? { items }
          : undefined;
      })()
    : undefined;
}

function parseAuditEventPage(value: unknown): AuditEventPage | undefined {
  if (
    !isRecord(value) ||
    !Array.isArray(value.items) ||
    (value.nextCursor !== null && typeof value.nextCursor !== "string")
  )
    return undefined;
  const items = value.items.map(parseAuditEvent);
  return items.every(
    (item): item is NonNullable<typeof item> => item !== undefined,
  )
    ? {
        items,
        nextCursor:
          typeof value.nextCursor === "string" ? value.nextCursor : null,
      }
    : undefined;
}

function parseAuditEvent(
  value: unknown,
): AuditEventPage["items"][number] | undefined {
  if (
    !isRecord(value) ||
    typeof value.eventId !== "string" ||
    !isNonNegativeInteger(value.occurredAt) ||
    typeof value.actorId !== "string" ||
    typeof value.commandId !== "string" ||
    typeof value.correlationId !== "string" ||
    !["succeeded", "failed", "denied"].includes(String(value.outcome)) ||
    !isRecord(value.resource) ||
    typeof value.resource.resourceType !== "string" ||
    typeof value.resource.resourceId !== "string" ||
    !isRecord(value.resource.scope) ||
    !isRecord(value.metadata)
  )
    return undefined;
  const scope = parseResourceScope(value.resource.scope);
  if (!scope) return undefined;
  if (Object.values(value.metadata).some((item) => typeof item !== "string"))
    return undefined;
  return {
    eventId: value.eventId,
    occurredAt: value.occurredAt,
    actorId: value.actorId,
    resource: {
      resourceType: value.resource.resourceType,
      resourceId: value.resource.resourceId,
      scope,
    },
    commandId: value.commandId,
    correlationId: value.correlationId,
    outcome: value.outcome as AuditEventPage["items"][number]["outcome"],
    metadata: value.metadata as Record<string, string>,
  };
}

function parseProxyRecord(value: unknown): ProxyRecord | undefined {
  if (
    !isRecord(value) ||
    typeof value.workspaceId !== "string" ||
    !isPositiveInteger(value.revision)
  )
    return undefined;
  const proxy = parseProxyView(value.proxy);
  return proxy
    ? { workspaceId: value.workspaceId, proxy, revision: value.revision }
    : undefined;
}

function parseProfileProxyAssignment(
  value: unknown,
): ProfileProxyAssignment | undefined {
  if (
    !isRecord(value) ||
    typeof value.profileId !== "string" ||
    value.profileId.length === 0 ||
    typeof value.workspaceId !== "string" ||
    value.workspaceId.length === 0 ||
    !isPositiveInteger(value.expectedProfileRevision) ||
    (value.proxyId !== null &&
      (typeof value.proxyId !== "string" || value.proxyId.length === 0)) ||
    typeof value.requestedAt !== "string" ||
    !Number.isFinite(Date.parse(value.requestedAt))
  )
    return undefined;
  return {
    profileId: value.profileId,
    workspaceId: value.workspaceId,
    expectedProfileRevision: value.expectedProfileRevision,
    proxyId: value.proxyId,
    requestedAt: value.requestedAt,
  };
}

function parseGroup(value: unknown): Group | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    value.id.length === 0 ||
    typeof value.name !== "string" ||
    value.name.length === 0 ||
    typeof value.createdAt !== "string" ||
    !Number.isFinite(Date.parse(value.createdAt)) ||
    typeof value.updatedAt !== "string" ||
    !Number.isFinite(Date.parse(value.updatedAt))
  )
    return undefined;
  const appearance =
    value.appearance === undefined
      ? undefined
      : value.appearance === null
        ? null
        : isRecord(value.appearance)
          ? parseJsonValue(value.appearance)
          : undefined;
  if (appearance === undefined && value.appearance !== undefined)
    return undefined;
  if (
    appearance !== undefined &&
    appearance !== null &&
    (Array.isArray(appearance) || typeof appearance !== "object")
  )
    return undefined;
  return {
    id: value.id,
    name: value.name,
    ...(appearance === undefined ? {} : { appearance }),
    createdAt: value.createdAt,
    updatedAt: value.updatedAt,
  };
}

function parseGroupRecord(value: unknown): GroupRecord | undefined {
  if (
    !isRecord(value) ||
    typeof value.workspaceId !== "string" ||
    value.workspaceId.length === 0 ||
    !isPositiveInteger(value.revision)
  )
    return undefined;
  const group = parseGroup(value.group);
  return group
    ? { group, workspaceId: value.workspaceId, revision: value.revision }
    : undefined;
}

function parseProfileRecord(value: unknown): ProfileRecord | undefined {
  if (!isRecord(value) || !isPositiveInteger(value.revision)) return undefined;
  const profile = parseProfile(value);
  return profile ? { ...profile, revision: value.revision } : undefined;
}

function parseProxyView(value: unknown): ProxyView | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    !isProxyProtocol(value.protocol) ||
    typeof value.host !== "string" ||
    !isPositiveInteger(value.port) ||
    value.port > 65535 ||
    typeof value.hasCredential !== "boolean" ||
    typeof value.rotationConfigured !== "boolean" ||
    !isStringArray(value.bypassRules) ||
    typeof value.createdAt !== "string" ||
    typeof value.updatedAt !== "string" ||
    (value.name !== undefined && typeof value.name !== "string")
  )
    return undefined;
  return {
    id: value.id,
    ...(typeof value.name === "string" ? { name: value.name } : {}),
    protocol: value.protocol,
    host: value.host,
    port: value.port,
    hasCredential: value.hasCredential,
    rotationConfigured: value.rotationConfigured,
    bypassRules: value.bypassRules,
    createdAt: value.createdAt,
    updatedAt: value.updatedAt,
  };
}

function parseProxyCreateResult(value: unknown): ProxyCreateResult | undefined {
  if (!isRecord(value) || typeof value.deduplicated !== "boolean")
    return undefined;
  const record = parseProxyRecord(value.record);
  return record ? { record, deduplicated: value.deduplicated } : undefined;
}

function parseVersionedProfile(value: unknown): VersionedProfile | undefined {
  if (
    !isRecord(value) ||
    !isPositiveInteger(value.schemaVersion) ||
    !isPositiveInteger(value.revision)
  )
    return undefined;
  const profile = parseProfile(value.value);
  return profile
    ? {
        schemaVersion: value.schemaVersion,
        revision: value.revision,
        value: profile,
      }
    : undefined;
}

function parseVersionedProfilePage(
  value: unknown,
): VersionedProfilePage | undefined {
  if (
    !isRecord(value) ||
    !Array.isArray(value.items) ||
    (typeof value.nextCursor !== "string" && value.nextCursor !== null)
  )
    return undefined;
  const items = value.items.map(parseVersionedProfile);
  return items.every((item): item is VersionedProfile => item !== undefined)
    ? { items, nextCursor: value.nextCursor }
    : undefined;
}

function parseProfile(value: unknown): Profile | undefined {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    typeof value.name !== "string" ||
    !isRuntime(value.runtime) ||
    !isProfileState(value.state) ||
    typeof value.archived !== "boolean" ||
    typeof value.createdAt !== "string" ||
    typeof value.updatedAt !== "string" ||
    !isStringArray(value.groupIds) ||
    !isStringArray(value.tags)
  )
    return undefined;
  const runtimeConfig = parseJsonValue(value.runtimeConfig);
  const fingerprint = parseJsonValue(value.fingerprint);
  if (
    !isStringOrNull(value.runtimeVersion) ||
    runtimeConfig === undefined ||
    fingerprint === undefined ||
    !isStringOrNull(value.note) ||
    !isStringOrNull(value.proxyId) ||
    !isStringOrNull(value.vpnId) ||
    !isStringOrNull(value.extensionGroupId) ||
    !isEnumOrNull(value.syncMode, isSyncMode)
  )
    return undefined;
  return {
    id: value.id,
    name: value.name,
    runtime: value.runtime,
    runtimeVersion:
      typeof value.runtimeVersion === "string" ? value.runtimeVersion : null,
    runtimeConfig,
    state: value.state,
    archived: value.archived,
    fingerprint,
    note: typeof value.note === "string" ? value.note : null,
    proxyId: typeof value.proxyId === "string" ? value.proxyId : null,
    vpnId: typeof value.vpnId === "string" ? value.vpnId : null,
    groupIds: value.groupIds,
    tags: value.tags,
    extensionGroupId:
      typeof value.extensionGroupId === "string"
        ? value.extensionGroupId
        : null,
    syncMode: isSyncMode(value.syncMode) ? value.syncMode : null,
    createdAt: value.createdAt,
    updatedAt: value.updatedAt,
  };
}

function parseOperation(value: unknown): Operation | undefined {
  if (
    !isRecord(value) ||
    typeof value.operationId !== "string" ||
    typeof value.commandId !== "string" ||
    !isOperationStatus(value.status) ||
    (value.progress !== null &&
      (typeof value.progress !== "number" ||
        !Number.isFinite(value.progress) ||
        value.progress < 0 ||
        value.progress > 1))
  )
    return undefined;
  const scope = parseResourceScope(value.scope);
  if (!scope) return undefined;
  const error =
    value.error === undefined ? undefined : parseProblem(value.error);
  if (value.error !== undefined && !error) return undefined;
  return {
    operationId: value.operationId,
    commandId: value.commandId,
    scope,
    status: value.status,
    progress: value.progress,
    ...(value.result === undefined ? {} : { result: value.result }),
    ...(error ? { error } : {}),
  };
}

function parseResourceScope(value: unknown): ResourceScope | undefined {
  if (!isRecord(value)) return undefined;
  const accountId = value.accountId;
  const organizationId = value.organizationId;
  const workspaceId = value.workspaceId;
  const teamId = value.teamId;
  const memberId = value.memberId;
  const resourceId = value.resourceId;
  const profileId = value.profileId;
  const ids = [
    accountId,
    organizationId,
    workspaceId,
    teamId,
    memberId,
    resourceId,
    profileId,
  ];
  if (!ids.every((id) => id === null || typeof id === "string"))
    return undefined;
  return {
    accountId,
    organizationId,
    workspaceId,
    teamId,
    memberId,
    resourceId,
    profileId,
  } as ResourceScope;
}

function parseProxyParseReport(value: unknown): ProxyParseReport | undefined {
  if (
    !isRecord(value) ||
    !isNonNegativeInteger(value.validCount) ||
    !isNonNegativeInteger(value.invalidCount) ||
    !isNonNegativeInteger(value.duplicateCount)
  )
    return undefined;
  const rows = Array.isArray(value.rows)
    ? value.rows.map(parseProxyParseRow)
    : undefined;
  return rows?.every(
    (row): row is ProxyParseReport["rows"][number] => row !== undefined,
  )
    ? {
        validCount: value.validCount,
        invalidCount: value.invalidCount,
        duplicateCount: value.duplicateCount,
        rows,
      }
    : undefined;
}

function parseProxyParseRow(
  value: unknown,
): ProxyParseReport["rows"][number] | undefined {
  if (
    !isRecord(value) ||
    typeof value.valid !== "boolean" ||
    !optionalStringOrNull(value.protocol) ||
    !optionalStringOrNull(value.endpoint)
  )
    return undefined;
  if (value.line !== undefined && !isNonNegativeInteger(value.line))
    return undefined;
  if (value.error !== undefined && typeof value.error !== "string")
    return undefined;
  return {
    protocol: typeof value.protocol === "string" ? value.protocol : null,
    endpoint: typeof value.endpoint === "string" ? value.endpoint : null,
    valid: value.valid,
    ...(typeof value.line === "number" ? { line: value.line } : {}),
    ...(typeof value.error === "string" ? { error: value.error } : {}),
  };
}

function parseArray<T>(
  value: unknown,
  parse: (item: unknown) => T | undefined,
): T[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const items = value.map(parse);
  return items.every((item): item is T => item !== undefined)
    ? items
    : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) && value.every((item) => typeof item === "string")
  );
}
function optionalStringOrNull(value: unknown): boolean {
  return value === undefined || value === null || typeof value === "string";
}
function isStringOrNull(value: unknown): boolean {
  return value === null || typeof value === "string";
}
function isEnumOrNull<T>(
  value: unknown,
  guard: (value: unknown) => value is T,
): boolean {
  return value === null || guard(value);
}
function parseJsonValue(value: unknown): JsonValue | undefined {
  return isJsonValue(value) ? value : undefined;
}
function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean")
    return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return isRecord(value) && Object.values(value).every(isJsonValue);
}
function isRuntime(value: unknown): value is Runtime {
  return typeof value === "string" && RUNTIME_VALUES.includes(value as Runtime);
}
function isProfileState(value: unknown): value is ProfileState {
  return (
    typeof value === "string" &&
    PROFILE_STATE_VALUES.includes(value as ProfileState)
  );
}
function isSyncMode(value: unknown): value is SyncMode {
  return (
    typeof value === "string" && SYNC_MODE_VALUES.includes(value as SyncMode)
  );
}
function isProxyProtocol(
  value: unknown,
): value is import("./api.ts").ProxyProtocol {
  return (
    typeof value === "string" &&
    PROXY_PROTOCOL_VALUES.includes(value as import("./api.ts").ProxyProtocol)
  );
}
function isOperationStatus(value: unknown): value is OperationStatus {
  return (
    value === "queued" ||
    value === "running" ||
    value === "completed" ||
    value === "failed" ||
    value === "cancelled"
  );
}
function isMemberRole(value: unknown): value is MemberSummary["role"] {
  return (
    typeof value === "string" &&
    MEMBER_ROLE_VALUES.includes(value as MemberSummary["role"])
  );
}
function isMemberStatus(value: unknown): value is MemberSummary["status"] {
  return (
    typeof value === "string" &&
    MEMBER_STATUS_VALUES.includes(value as MemberSummary["status"])
  );
}
function isResourceKind(value: unknown): value is ResourceSummary["kind"] {
  return (
    typeof value === "string" &&
    RESOURCE_KIND_VALUES.includes(value as ResourceSummary["kind"])
  );
}
function isResourceState(value: unknown): value is ResourceSummary["state"] {
  return (
    typeof value === "string" &&
    RESOURCE_STATE_VALUES.includes(value as ResourceSummary["state"])
  );
}
function isGrantAction(value: unknown): value is GrantSummary["action"] {
  return (
    typeof value === "string" &&
    ACTION_VALUES.includes(value as GrantSummary["action"])
  );
}
function isGrantEffect(value: unknown): value is GrantSummary["effect"] {
  return (
    typeof value === "string" &&
    EFFECT_VALUES.includes(value as GrantSummary["effect"])
  );
}

function validIdempotencyKey(value: string, correlationId: string): string {
  if (/^[\x20-\x7E]{16,128}$/.test(value)) return value;
  throw new WebApiError(
    problem(
      "idempotency_key_invalid",
      "Valid idempotency key required",
      400,
      correlationId,
    ),
  );
}

async function responseProblem(
  response: Response,
  correlationId: string,
): Promise<Problem> {
  try {
    const parsed = parseProblem(await response.json());
    if (parsed) return parsed;
  } catch {}
  return problem(
    "http_error",
    response.statusText || "Request failed",
    response.status,
    correlationId,
  );
}

function parseProblem(value: unknown): Problem | undefined {
  if (
    !isRecord(value) ||
    typeof value.type !== "string" ||
    typeof value.title !== "string" ||
    !Number.isInteger(value.status) ||
    typeof value.code !== "string" ||
    typeof value.correlationId !== "string" ||
    typeof value.retryable !== "boolean" ||
    !isRecord(value.fields)
  )
    return undefined;
  if (
    (value.detail !== undefined && typeof value.detail !== "string") ||
    (value.instance !== undefined && typeof value.instance !== "string")
  )
    return undefined;
  return value as Problem;
}

function problem(
  code: string,
  title: string,
  status: number,
  correlationId: string,
): Problem {
  return {
    type: `https://buglogin.local/problems/${code}`,
    title,
    status,
    code,
    correlationId,
    retryable: false,
    fields: {},
  };
}

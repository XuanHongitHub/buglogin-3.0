export type PublicSummary = {
  release: { version: string; publishedAt: string; downloadUrl: string } | null;
  status: { label: string; detail: string };
};

export type LoadState<T> =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'empty'; message: string }
  | { status: 'ready'; data: T };

export type AccountSummary = {
  accountId: string;
  status?: string;
  revision?: number;
  createdAt?: string;
  updatedAt?: string;
};

export type WebSession = { accountId: string; expiresAt: number };
export type WebSessionSummary = { id: string; kind: 'browser' | 'desktop' | 'api'; deviceId: string | null; createdAt: number | null; expiresAt: number; revokedAt: number | null; current: boolean };
export type ApiTokenSummary = { id: string; name: string; scopes: string[]; createdAt: number; expiresAt: number; revokedAt: number | null };
export type ApiTokenCreateInput = { name: string; scopes: string[]; expiresAt: number };

export type Runtime = 'wayfern' | 'bugox' | 'bugium' | 'bugmini' | 'bugthor' | 'bughelium';
export type ProfileState = 'created' | 'ready' | 'launching' | 'running' | 'stopping' | 'failed' | 'recovering';
export type SyncMode = 'local' | 'managed' | 'disabled';
export type Profile = {
  id: string;
  name: string;
  runtime: Runtime;
  runtimeVersion: string | null;
  runtimeConfig: unknown | null;
  state: ProfileState;
  archived: boolean;
  fingerprint: unknown | null;
  note: string | null;
  proxyId: string | null;
  vpnId: string | null;
  groupIds: string[];
  tags: string[];
  extensionGroupId: string | null;
  syncMode: SyncMode | null;
  createdAt: string;
  updatedAt: string;
};
export type VersionedProfile = { revision: number; value: Profile };
export type ProfileCreateInput = { name: string; runtime: Runtime; runtimeVersion?: string; workspaceId?: string; proxyId?: string };
export type LaunchInput = { profilesRoot: string; extraArgs: string[] };
export type OperationStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
export type Operation = { operationId: string; commandId: string; scope: unknown; status: OperationStatus; progress: number | null; result?: unknown; error?: Problem };
export type MigrationClass = 'direct' | 'partial' | 'unsupported' | 'unknown';
export type MigrationSourceIdentity = { system: string; version: string; instanceId: string; os: string; architecture: string; root: string };
export type MigrationRuntimeFinding = { runtime: string; class: MigrationClass; reasonCode: string; records: number };
export type MigrationRecordIssue = { sourceRef: string; code: string };
export type MigrationInspectReport = {
  schema: string;
  source: MigrationSourceIdentity;
  entityCounts: Record<string, number>;
  totalBytes: number;
  runtimeFindings: MigrationRuntimeFinding[];
  secretScan: { filesWithSecretFields: number; secretFieldsPresent: number };
  invalidRecords: MigrationRecordIssue[];
  warnings: MigrationRecordIssue[];
  danglingReferences: MigrationRecordIssue[];
  sourceTreeSha256: string;
  cutoverEligibility: 'eligible' | 'blocked';
};
export type MigrationSnapshotEntry = { path: string; bytes: number; sha256: string };
export type MigrationSourceSnapshot = { snapshotId: string; sourceTreeSha256: string; totalBytes: number; files: MigrationSnapshotEntry[] };
export type MigrationInspectResult = { report: MigrationInspectReport; snapshot: MigrationSourceSnapshot };
export type MigrationInspectInput = { sourceRoot: string };
export type MigrationImportInput = { sourceRoot: string; stagingParent: string; destinationRoot: string; runId: string; confirmation: string };
export type ProxyParseRow = { protocol: string | null; endpoint: string | null; valid: boolean; line?: number; error?: string };
export type ProxyParseReport = { validCount: number; invalidCount: number; duplicateCount: number; rows: ProxyParseRow[] };
export type ProxyProtocol = 'http' | 'https' | 'socks4' | 'socks5';
export type ProxyView = { id: string; name?: string; protocol: ProxyProtocol; host: string; port: number; hasCredential: boolean; rotationConfigured: boolean; bypassRules: string[]; createdAt: string; updatedAt: string };
export type ProxyRecord = { workspaceId: string; proxy: ProxyView; revision: number };
export type ProxyPage = { items: ProxyRecord[] };
export type ProxyMetadataInput = { id: string; name?: string; protocol: ProxyProtocol; host: string; port: number; bypassRules: string[]; createdAt: string; updatedAt: string };
export type ProxyCreateInput = { workspaceId: string; proxy: ProxyMetadataInput };
export type ProxyUpdateInput = { workspaceId: string; proxyId: string; expectedRevision: number; proxy: ProxyMetadataInput };
export type ProxyDeleteInput = { workspaceId: string; proxyId: string; expectedRevision: number };
export type ProxyCreateResult = { record: ProxyRecord; deduplicated: boolean };

export type OrganizationSummary = {
  id: string;
  accountId: string;
  name: string;
  createdAt: string;
};

export type OrganizationPolicy = {
  maxWorkspaces: number;
  offlineEntitled: boolean;
};

export type WorkspaceSummary = {
  id: string;
  organizationId: string;
  name: string;
  policy: {
    maxProfiles: number;
    maxMembers: number;
    maxStorageBytes: number;
    offlineEntitled: boolean;
  };
  createdAt: string;
};

export type TeamSummary = { id: string; workspaceId: string; name: string; memberIds: string[] };
export type TeamRecord = { team: TeamSummary; revision: number };
export type MemberRole = 'owner' | 'admin' | 'operator' | 'member';
export type MemberStatus = 'invited' | 'active' | 'suspended';
export type MemberSummary = { id: string; accountId: string; workspaceId: string; teamId: string | null; role: MemberRole; status: MemberStatus; inviteExpiresAt: number | null };
export type ResourceSummary = { id: string; workspaceId: string; kind: string; state: string; displayName: string };
export type GrantSummary = { id: string; workspaceId: string; resourceId: string; memberId: string; action: string; effect: string };
export type GrantCreateInput = GrantSummary;
export type GrantRevocationReceipt = { grantId: string; revoked: true };

export type MutationOptions = { idempotencyKey: string; signal: AbortSignal };
export type OrganizationCreateInput = { organization: OrganizationSummary; policy: OrganizationPolicy };
export type OrganizationUpdateInput = OrganizationCreateInput & { expectedRevision: number };
export type WorkspaceUpdateInput = { workspace: WorkspaceSummary; expectedRevision: number };
export type TeamUpdateInput = { name: string; expectedRevision: number };
export type TeamDeleteInput = { expectedRevision: number };
export type MemberRoleUpdateInput = { role: MemberRole };

export type WorkspaceOperationalData = {
  workspace: WorkspaceSummary;
  teams: TeamSummary[];
  members: MemberSummary[];
  resources: ResourceSummary[];
  grants: GrantSummary[];
};

export type AdminSummary = {
  account: AccountSummary;
  organizations: OrganizationSummary[];
  workspaces: WorkspaceOperationalData[];
};

export type Problem = {
  type: string;
  title: string;
  status: number;
  code: string;
  detail?: string;
  instance?: string;
  correlationId: string;
  retryable: boolean;
  fields: Record<string, unknown>;
};

export class WebApiError extends Error {
  readonly problem: Problem;

  constructor(problem: Problem) {
    super(problem.detail ?? problem.title);
    this.name = 'WebApiError';
    this.problem = problem;
  }
}

export type WebApiClient = {
  auth: {
    login(identifier: string, password: string, signal: AbortSignal): Promise<WebSession>;
    session(signal: AbortSignal): Promise<WebSession>;
    sessions(signal: AbortSignal): Promise<WebSessionSummary[]>;
    apiTokens(signal: AbortSignal): Promise<ApiTokenSummary[]>;
    createApiToken(input: ApiTokenCreateInput, signal: AbortSignal): Promise<{ token: string; tokenRecord: ApiTokenSummary }>;
    revokeApiToken(tokenId: string, signal: AbortSignal): Promise<void>;
    requestRecovery(identifier: string, signal: AbortSignal): Promise<void>;
    completeRecovery(token: string, newPassword: string, idempotencyKey: string, signal: AbortSignal): Promise<void>;
    approveDevice(userCode: string, signal: AbortSignal): Promise<void>;
    revokeSession(sessionId: string, signal: AbortSignal): Promise<void>;
    logout(signal: AbortSignal): Promise<void>;
  };
  reads: {
    publicSummary(signal: AbortSignal): Promise<PublicSummary>;
    account(signal: AbortSignal): Promise<AccountSummary>;
    organizations(signal: AbortSignal): Promise<OrganizationSummary[]>;
    workspaces(signal: AbortSignal): Promise<WorkspaceSummary[]>;
    workspace(id: string, signal: AbortSignal): Promise<WorkspaceSummary>;
    workspaceOperational(id: string, signal: AbortSignal): Promise<WorkspaceOperationalData>;
    adminSummary(signal: AbortSignal): Promise<AdminSummary>;
    profiles(signal: AbortSignal): Promise<VersionedProfile[]>;
    operation(operationId: string, signal: AbortSignal): Promise<Operation>;
    proxies(workspaceId: string, signal: AbortSignal): Promise<ProxyPage>;
    proxy(workspaceId: string, proxyId: string, signal: AbortSignal): Promise<ProxyRecord>;
  };
  writes: {
    createOrganization(input: OrganizationCreateInput, options: MutationOptions): Promise<void>;
    updateOrganization(id: string, input: OrganizationUpdateInput, options: MutationOptions): Promise<OrganizationSummary>;
    createWorkspace(organizationId: string, workspace: WorkspaceSummary, options: MutationOptions): Promise<WorkspaceSummary>;
    updateWorkspace(id: string, input: WorkspaceUpdateInput, options: MutationOptions): Promise<WorkspaceSummary>;
    createTeam(workspaceId: string, team: TeamSummary, options: MutationOptions): Promise<TeamRecord>;
    updateTeam(workspaceId: string, teamId: string, input: TeamUpdateInput, options: MutationOptions): Promise<TeamRecord>;
    deleteTeam(workspaceId: string, teamId: string, input: TeamDeleteInput, options: MutationOptions): Promise<TeamRecord>;
    inviteMember(workspaceId: string, member: MemberSummary, options: MutationOptions): Promise<MemberSummary>;
    activateMember(workspaceId: string, memberId: string, options: MutationOptions): Promise<MemberSummary>;
    suspendMember(workspaceId: string, memberId: string, options: MutationOptions): Promise<MemberSummary>;
    updateMemberRole(workspaceId: string, memberId: string, input: MemberRoleUpdateInput, options: MutationOptions): Promise<MemberSummary>;
    createGrant(workspaceId: string, input: GrantCreateInput, options: MutationOptions): Promise<GrantSummary>;
    revokeGrant(workspaceId: string, grantId: string, options: MutationOptions): Promise<GrantRevocationReceipt>;
    createProfile(input: ProfileCreateInput, options: MutationOptions): Promise<VersionedProfile>;
    launchProfile(profileId: string, input: LaunchInput, options: MutationOptions): Promise<Operation>;
    stopProfile(profileId: string, options: MutationOptions): Promise<Operation>;
    parseProxies(text: string, signal: AbortSignal): Promise<ProxyParseReport>;
    createProxy(input: ProxyCreateInput, options: MutationOptions): Promise<ProxyCreateResult>;
    updateProxy(input: ProxyUpdateInput, options: MutationOptions): Promise<ProxyRecord>;
    deleteProxy(input: ProxyDeleteInput, options: MutationOptions): Promise<ProxyRecord>;
    inspectMigration(input: MigrationInspectInput, options: MutationOptions): Promise<Operation>;
    importMigration(input: MigrationImportInput, options: MutationOptions): Promise<Operation>;
  };
};

export function errorMessage(error: unknown): string {
  return error instanceof WebApiError ? error.problem.detail ?? error.problem.title : error instanceof Error ? error.message : 'The request failed.';
}

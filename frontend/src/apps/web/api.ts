import type {
  Account as AccountDto,
  AccountStatus,
  AccountView,
  ApiToken,
  ApiTokenCreateInput,
  AuditEvent,
  AuditEventPage,
  GrantCreateInput,
  Grant as GrantSummary,
  Group,
  GroupCreateInput,
  GroupDeleteInput,
  GroupRecord,
  GroupRecordList,
  GroupUpdateInput,
  JsonValue,
  MemberRole,
  MemberRoleUpdateBody,
  MemberStatus,
  Member as MemberSummary,
  Operation,
  OperationStatus,
  OrganizationCreateInput,
  OrganizationPolicy,
  OrganizationRecord,
  Organization as OrganizationSummary,
  OrganizationUpdate as OrganizationUpdateInput,
  Problem,
  Profile,
  ProfileCreateInput,
  ProfileCreateRuntime,
  ProfileGroupsReplaceInput,
  ProfileLaunchInput,
  ProfileProxyAssignment,
  ProfileProxyAssignmentGetInput,
  ProfileProxyAssignmentSetInput,
  ProfileRecord,
  ProfileState,
  ProxyCreateInput,
  ProxyCreateResult,
  ProxyDeleteInput,
  ProxyMetadataInput,
  ProxyPage,
  ProxyRecord,
  ProxyUpdateInput,
  ProxyView,
  ResourceScope,
  Resource as ResourceSummary,
  Runtime,
  WebSession as SessionSummaryDto,
  SyncMode,
  TeamRecord,
  Team as TeamSummary,
  VersionedProfile,
  VersionedProfilePage,
  WebSessionResponse,
  WorkspacePolicy,
  Workspace as WorkspaceSummary,
  WorkspaceUpdate as WorkspaceUpdateInput,
} from "../../generated/command-registry.js";

export type {
  AccountDto,
  AccountStatus,
  AccountView,
  ApiToken,
  ApiTokenCreateInput,
  AuditEvent,
  AuditEventPage,
  GrantCreateInput,
  GrantSummary,
  Group,
  GroupCreateInput,
  GroupDeleteInput,
  GroupRecord,
  GroupRecordList,
  GroupUpdateInput,
  JsonValue,
  MemberRole,
  MemberRoleUpdateBody,
  MemberStatus,
  MemberSummary,
  Operation,
  OperationStatus,
  OrganizationCreateInput,
  OrganizationPolicy,
  OrganizationRecord,
  OrganizationSummary,
  OrganizationUpdateInput,
  Problem,
  Profile,
  ProfileCreateInput,
  ProfileCreateRuntime,
  ProfileGroupsReplaceInput,
  ProfileLaunchInput,
  ProfileProxyAssignment,
  ProfileProxyAssignmentGetInput,
  ProfileProxyAssignmentSetInput,
  ProfileRecord,
  ProfileState,
  ProxyCreateInput,
  ProxyCreateResult,
  ProxyDeleteInput,
  ProxyMetadataInput,
  ProxyPage,
  ProxyRecord,
  ProxyUpdateInput,
  ProxyView,
  ResourceScope,
  ResourceSummary,
  Runtime,
  SyncMode,
  TeamRecord,
  TeamSummary,
  VersionedProfile,
  VersionedProfilePage,
  WebSessionResponse,
  WorkspacePolicy,
  WorkspaceSummary,
  WorkspaceUpdateInput,
};

export type PublicSummary = {
  release: { version: string; publishedAt: string; downloadUrl: string } | null;
  status: { label: string; detail: string };
};

export type LoadState<T> =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "empty"; message: string }
  | { status: "ready"; data: T };

export type AccountSummary = AccountView & Partial<Omit<AccountDto, "id">>;

export type WebSession = WebSessionResponse;
export type WebSessionSummary = SessionSummaryDto;
export type ApiTokenSummary = ApiToken;

export type LaunchInput = ProfileLaunchInput;
export type MigrationClass = "direct" | "partial" | "unsupported" | "unknown";
export type MigrationSourceIdentity = {
  system: string;
  version: string;
  instanceId: string;
  os: string;
  architecture: string;
  root: string;
};
export type MigrationRuntimeFinding = {
  runtime: string;
  class: MigrationClass;
  reasonCode: string;
  records: number;
};
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
  cutoverEligibility: "eligible" | "blocked";
};
export type MigrationSnapshotEntry = {
  path: string;
  bytes: number;
  sha256: string;
};
export type MigrationSourceSnapshot = {
  snapshotId: string;
  sourceTreeSha256: string;
  totalBytes: number;
  files: MigrationSnapshotEntry[];
};
export type MigrationInspectResult = {
  report: MigrationInspectReport;
  snapshot: MigrationSourceSnapshot;
};
export type MigrationInspectInput = { sourceRoot: string };
export type MigrationImportInput = {
  sourceRoot: string;
  stagingParent: string;
  destinationRoot: string;
  runId: string;
  confirmation: string;
};
export type ProxyParseRow = {
  protocol: string | null;
  endpoint: string | null;
  valid: boolean;
  line?: number;
  error?: string;
};
export type ProxyParseReport = {
  validCount: number;
  invalidCount: number;
  duplicateCount: number;
  rows: ProxyParseRow[];
};
export type ProxyProtocol = ProxyMetadataInput["protocol"];

export type GrantRevocationReceipt = { grantId: string; revoked: true };

export type MutationOptions = { idempotencyKey: string; signal: AbortSignal };
export type TeamUpdateInput = { name: string; expectedRevision: number };
export type TeamDeleteInput = { expectedRevision: number };
export type MemberRoleUpdateInput = MemberRoleUpdateBody;

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

export class WebApiError extends Error {
  readonly problem: Problem;

  constructor(problem: Problem) {
    super(problem.detail ?? problem.title);
    this.name = "WebApiError";
    this.problem = problem;
  }
}

export type WebApiClient = {
  auth: {
    login(
      identifier: string,
      password: string,
      signal: AbortSignal,
    ): Promise<WebSession>;
    session(signal: AbortSignal): Promise<WebSession>;
    sessions(signal: AbortSignal): Promise<WebSessionSummary[]>;
    apiTokens(signal: AbortSignal): Promise<ApiTokenSummary[]>;
    createApiToken(
      input: ApiTokenCreateInput,
      signal: AbortSignal,
    ): Promise<{ token: string; tokenRecord: ApiTokenSummary }>;
    revokeApiToken(tokenId: string, signal: AbortSignal): Promise<void>;
    requestRecovery(identifier: string, signal: AbortSignal): Promise<void>;
    completeRecovery(
      token: string,
      newPassword: string,
      idempotencyKey: string,
      signal: AbortSignal,
    ): Promise<void>;
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
    workspaceOperational(
      id: string,
      signal: AbortSignal,
    ): Promise<WorkspaceOperationalData>;
    adminSummary(signal: AbortSignal): Promise<AdminSummary>;
    profiles(
      cursor: string | null,
      limit: number,
      signal: AbortSignal,
    ): Promise<VersionedProfilePage>;
    operation(operationId: string, signal: AbortSignal): Promise<Operation>;
    proxies(workspaceId: string, signal: AbortSignal): Promise<ProxyPage>;
    proxy(
      workspaceId: string,
      proxyId: string,
      signal: AbortSignal,
    ): Promise<ProxyRecord>;
    profileProxyAssignment(
      input: ProfileProxyAssignmentGetInput,
      signal: AbortSignal,
    ): Promise<ProfileProxyAssignment>;
    groups(workspaceId: string, signal: AbortSignal): Promise<GroupRecordList>;
    group(
      workspaceId: string,
      groupId: string,
      signal: AbortSignal,
    ): Promise<GroupRecord>;
    auditEvents(
      workspaceId: string,
      cursor: string | null,
      limit: number,
      signal: AbortSignal,
    ): Promise<AuditEventPage>;
  };
  writes: {
    createOrganization(
      input: OrganizationCreateInput,
      options: MutationOptions,
    ): Promise<OrganizationRecord>;
    updateOrganization(
      id: string,
      input: OrganizationUpdateInput,
      options: MutationOptions,
    ): Promise<OrganizationSummary>;
    createWorkspace(
      organizationId: string,
      workspace: WorkspaceSummary,
      options: MutationOptions,
    ): Promise<WorkspaceSummary>;
    updateWorkspace(
      id: string,
      input: WorkspaceUpdateInput,
      options: MutationOptions,
    ): Promise<WorkspaceSummary>;
    createTeam(
      workspaceId: string,
      team: TeamSummary,
      options: MutationOptions,
    ): Promise<TeamRecord>;
    updateTeam(
      workspaceId: string,
      teamId: string,
      input: TeamUpdateInput,
      options: MutationOptions,
    ): Promise<TeamRecord>;
    deleteTeam(
      workspaceId: string,
      teamId: string,
      input: TeamDeleteInput,
      options: MutationOptions,
    ): Promise<TeamRecord>;
    inviteMember(
      workspaceId: string,
      member: MemberSummary,
      options: MutationOptions,
    ): Promise<MemberSummary>;
    activateMember(
      workspaceId: string,
      memberId: string,
      options: MutationOptions,
    ): Promise<MemberSummary>;
    suspendMember(
      workspaceId: string,
      memberId: string,
      options: MutationOptions,
    ): Promise<MemberSummary>;
    updateMemberRole(
      workspaceId: string,
      memberId: string,
      input: MemberRoleUpdateInput,
      options: MutationOptions,
    ): Promise<MemberSummary>;
    createGrant(
      workspaceId: string,
      input: GrantCreateInput,
      options: MutationOptions,
    ): Promise<GrantSummary>;
    revokeGrant(
      workspaceId: string,
      grantId: string,
      options: MutationOptions,
    ): Promise<GrantRevocationReceipt>;
    createProfile(
      input: ProfileCreateInput,
      options: MutationOptions,
    ): Promise<VersionedProfile>;
    launchProfile(
      profileId: string,
      input: LaunchInput,
      options: MutationOptions,
    ): Promise<Operation>;
    stopProfile(
      profileId: string,
      options: MutationOptions,
    ): Promise<Operation>;
    parseProxies(text: string, signal: AbortSignal): Promise<ProxyParseReport>;
    createProxy(
      input: ProxyCreateInput,
      options: MutationOptions,
    ): Promise<ProxyCreateResult>;
    updateProxy(
      input: ProxyUpdateInput,
      options: MutationOptions,
    ): Promise<ProxyRecord>;
    deleteProxy(
      input: ProxyDeleteInput,
      options: MutationOptions,
    ): Promise<ProxyRecord>;
    stageProfileProxyAssignment(
      input: ProfileProxyAssignmentSetInput,
      options: MutationOptions,
    ): Promise<ProfileProxyAssignment>;
    createGroup(
      input: GroupCreateInput,
      options: MutationOptions,
    ): Promise<GroupRecord>;
    updateGroup(
      input: GroupUpdateInput,
      options: MutationOptions,
    ): Promise<GroupRecord>;
    deleteGroup(
      input: GroupDeleteInput,
      options: MutationOptions,
    ): Promise<GroupRecord>;
    replaceProfileGroups(
      input: ProfileGroupsReplaceInput,
      options: MutationOptions,
    ): Promise<ProfileRecord>;
    inspectMigration(
      input: MigrationInspectInput,
      options: MutationOptions,
    ): Promise<Operation>;
    importMigration(
      input: MigrationImportInput,
      options: MutationOptions,
    ): Promise<Operation>;
  };
};

export function errorMessage(error: unknown): string {
  return error instanceof WebApiError
    ? (error.problem.detail ?? error.problem.title)
    : error instanceof Error
      ? error.message
      : "The request failed.";
}

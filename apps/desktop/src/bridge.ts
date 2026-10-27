import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  type Account,
  type AccountAdminRecord,
  type AccountCreatePendingInput,
  type AccountDraftPage,
  type AccountDraftsInput,
  type AccountIdentity,
  type AccountIdentityGetInput,
  type AccountIdentityUpdateCommandInput,
  type AccountPasswordChangeInput,
  type AccountPasswordChangeReceipt,
  type AccountProvisioningReceipt,
  type AccountSessionRevokeReceipt,
  type AccountStatusUpdateCommandInput,
  type AccountView,
  type ApiToken,
  type ApiTokenPage,
  COMMANDS,
  type CommandId,
  type DeleteReceipt,
  type DeviceEncryptionKey,
  type EventFilterInput,
  type ExtensionGroupCreateInput,
  type ExtensionGroupDeleteInput,
  type ExtensionGroupIdInput,
  type ExtensionGroupRecord,
  type ExtensionGroupRecordList,
  type ExtensionGroupUpdateInput,
  type GroupCreateInput,
  type GroupDeleteInput,
  type GroupIdInput,
  type GroupRecord,
  type GroupRecordList,
  type GroupShareCreateInput,
  type GroupShareGrant,
  type GroupShareGrantList,
  type GroupShareRevokeInput,
  type GroupUpdateInput,
  type ManagementBatchInput,
  type ManagementBatchReceipt,
  type Member,
  type MemberIdInput,
  type MemberInvitationCreateInput,
  type MemberInvitationCreateReceipt,
  type MemberInvitationIdInput,
  type MemberInvitationRedeemInput,
  type MemberInvitationRevocationReceipt,
  type MemberInviteInput,
  type MemberRemoveInput,
  type MemberRemoveReceipt,
  type MemberRoleUpdateInput,
  type MigrationProfileDataImportInput,
  type OrganizationAccountCreateInput,
  type OrganizationCreateInput,
  type OrganizationDeleteCommandInput,
  type OrganizationDeleteReceipt,
  type OrganizationIdInput,
  type OrganizationMember,
  type OrganizationMemberPage,
  type OrganizationMemberStatusUpdateInput,
  type OrganizationMemberWorkspaceAssignInput,
  type OrganizationOwnerTransferCommandInput,
  type OrganizationRecord,
  type OrganizationUpdateInput,
  type ProfileDeleteInput,
  type ProfileExtensionGroupSetInput,
  type ProfileGroupsReplaceInput,
  type ProfileIdInput,
  type ProfileKeyRotationExecuteInput,
  type ProfileKeyRotationRecipientDevicesList,
  type ProfileListInput,
  type ProfilePage,
  type ProfileProxyAssignment,
  type ProfileProxyAssignmentGetInput,
  type ProfileProxyAssignmentSetInput,
  type ProfileRecord,
  type ProfileResource,
  type ProfileShareRecipientDeviceKeysInput,
  type ProfileUpdateInput,
  type ProxyCheckInput,
  type ProxyCheckResult,
  type ProxyImportInput,
  type ProxyImportReport,
  type Resource,
  type ResourceIdInput,
  type ResourceListInput,
  type ResourceRegisterInput,
  type TeamCreateInput,
  type TeamIdInput,
  type TeamRecord,
  type TeamRecordList,
  type TeamUpdateInput,
  type WorkspaceAccountCreateInput,
  type WorkspaceCreateInput,
  type WorkspaceDeleteCommandInput,
  type WorkspaceDeleteReceipt,
  type WorkspaceIdInput,
  type WorkspaceRecord,
  type WorkspaceUpdateInput,
} from "../../../frontend/src/generated/command-registry.ts";
import type {
  CommandResult,
  CookieCopyInput,
  CookieImportInput,
  CookieListResult,
  CookieMutationResult,
  CookieStageInput,
  CookieStageReceipt,
  DesktopAuthStatus,
  DesktopIntegrationStatus,
  GrantPage,
  Operation,
  OperationAccepted,
  Organization,
  Page,
  Problem,
  Profile,
  ProfileCloneInput,
  ProfileCloneReceipt,
  ProfileCreateInput,
  ProfileShareInput,
  ProfileShareReceipt,
  ProfileShareRevocationReceipt,
  ProfileShareRevokeInput,
  ProxyCreateInput,
  ProxyCreateResult,
  ProxyDeleteInput,
  ProxyPage,
  ProxyParseReport,
  ProxyRecord,
  ProxyUpdateInput,
  ResourceGrant,
  RuntimeCatalog,
  Workspace,
} from "./contracts.js";

type Invoke = <Result>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<Result>;

export type DesktopStreamEvent = {
  workspaceId: string;
  streamId: string | null;
  name: string;
  event: unknown;
};

export type DesktopStreamError = {
  workspaceId: string;
  code: string;
  message: string;
  status: number;
  retryable: boolean;
};

export const DESKTOP_AUTH_STATUS_CHANGED_EVENT =
  "buglogin:desktop-auth-status-changed";

let authInvalidationDispatched = false;

type EventSubscriptionReceipt = {
  workspaceId: string;
  subscriptionId: string;
  active: true;
};

export interface DesktopApi {
  getSystemHealth(): Promise<{ status: string; service: string }>;
  getIntegrationStatus(): Promise<DesktopIntegrationStatus>;
  listApiTokens(): Promise<ApiTokenPage>;
  createApiToken(input: {
    name: string;
    scopes: string[];
    expiresAt: number;
  }): Promise<{ token: string; tokenRecord: ApiToken }>;
  revokeApiToken(tokenId: string): Promise<void>;
  getAccount(): Promise<AccountView>;
  getOwnAccountIdentity(): Promise<AccountIdentity>;
  getAccountIdentity(input: AccountIdentityGetInput): Promise<AccountIdentity>;
  updateOwnAccountIdentity(
    input: AccountIdentityUpdateCommandInput,
  ): Promise<AccountIdentity>;
  changeOwnAccountPassword(
    input: AccountPasswordChangeInput,
  ): Promise<AccountPasswordChangeReceipt>;
  listAccountSessions(
    accountId: string,
  ): Promise<
    import("../../../frontend/src/generated/command-registry.ts").WebSessionPage
  >;
  revokeAccountSession(
    accountId: string,
    sessionId: string,
  ): Promise<AccountSessionRevokeReceipt>;
  generateAccountDrafts(input: AccountDraftsInput): Promise<AccountDraftPage>;
  managementBatch(
    input: ManagementBatchInput & { idempotencyKey?: string },
    idempotencyKey?: string,
  ): Promise<ManagementBatchReceipt>;
  listAccounts(): Promise<Page<AccountAdminRecord>>;
  createPendingAccount(
    input: AccountCreatePendingInput,
    idempotencyKey?: string,
  ): Promise<AccountProvisioningReceipt>;
  createScopedPendingAccount(
    input: WorkspaceAccountCreateInput,
    idempotencyKey?: string,
  ): Promise<AccountProvisioningReceipt>;
  updateAccountStatus(input: AccountStatusUpdateCommandInput): Promise<Account>;
  subscribeEvents(
    input: EventFilterInput,
    onEvent: (event: DesktopStreamEvent) => void,
    onError?: (error: DesktopStreamError) => void,
  ): Promise<() => Promise<void>>;
  authStatus(): Promise<DesktopAuthStatus>;
  login(identifier: string, password: string): Promise<DesktopAuthStatus>;
  startBugLabSso(): Promise<number>;
  completeBugLabSso(state: string, token: string): Promise<DesktopAuthStatus>;
  currentDeepLinks(): Promise<string[]>;
  getRuntimeCatalog(): Promise<RuntimeCatalog>;
  logout(): Promise<DesktopAuthStatus>;
  listProfiles(
    includeArchived?: boolean,
    page?: Pick<ProfileListInput, "limit" | "cursor" | "workspaceId">,
  ): Promise<ProfilePage>;
  getProfileBinding(profileId: string): Promise<ProfileResource>;
  getProfileProxyAssignment(
    input: ProfileProxyAssignmentGetInput,
  ): Promise<ProfileProxyAssignment | null>;
  getProfile(input: ProfileIdInput): Promise<ProfileRecord>;
  deleteProfile(input: ProfileDeleteInput): Promise<DeleteReceipt>;
  listOrganizations(): Promise<Page<Organization>>;
  getOrganization(input: OrganizationIdInput): Promise<Organization>;
  getOrganizationRecord(
    input: OrganizationIdInput,
  ): Promise<OrganizationRecord>;
  listOrganizationMembers(
    organizationId: string,
  ): Promise<OrganizationMemberPage>;
  createOrganizationAccount(
    input: OrganizationAccountCreateInput,
    idempotencyKey?: string,
  ): Promise<AccountProvisioningReceipt>;
  updateOrganizationMemberStatus(
    input: OrganizationMemberStatusUpdateInput,
  ): Promise<OrganizationMember>;
  assignOrganizationMemberToWorkspace(
    input: OrganizationMemberWorkspaceAssignInput,
    idempotencyKey?: string,
  ): Promise<Member>;
  createOrganization(input: OrganizationCreateInput): Promise<Organization>;
  updateOrganization(input: OrganizationUpdateInput): Promise<Organization>;
  deleteOrganization(
    input: OrganizationDeleteCommandInput,
  ): Promise<OrganizationDeleteReceipt>;
  transferOrganizationOwner(
    input: OrganizationOwnerTransferCommandInput,
  ): Promise<Organization>;
  listWorkspaces(organizationId: string): Promise<Page<Workspace>>;
  getWorkspace(input: WorkspaceIdInput): Promise<Workspace>;
  getWorkspaceRecord(input: WorkspaceIdInput): Promise<WorkspaceRecord>;
  createWorkspace(input: WorkspaceCreateInput): Promise<Workspace>;
  updateWorkspace(input: WorkspaceUpdateBridgeInput): Promise<Workspace>;
  deleteWorkspace(
    input: WorkspaceDeleteCommandInput,
  ): Promise<WorkspaceDeleteReceipt>;
  listTeams(workspaceId: string): Promise<TeamRecordList>;
  getTeam(input: TeamIdInput): Promise<TeamRecord>;
  createTeam(input: TeamCreateInput): Promise<TeamRecord>;
  updateTeam(input: TeamUpdateInput): Promise<TeamRecord>;
  listResources(input: ResourceListInput): Promise<Page<Resource>>;
  getResource(input: ResourceIdInput): Promise<Resource>;
  registerResource(input: ResourceRegisterInput): Promise<Resource>;
  listGroups(workspaceId: string): Promise<GroupRecordList>;
  getGroup(input: GroupIdInput): Promise<GroupRecord>;
  createGroup(input: GroupCreateInput): Promise<GroupRecord>;
  updateGroup(input: GroupUpdateInput): Promise<GroupRecord>;
  deleteGroup(input: GroupDeleteInput): Promise<GroupRecord>;
  listExtensionGroups(workspaceId: string): Promise<ExtensionGroupRecordList>;
  getExtensionGroup(
    input: ExtensionGroupIdInput,
  ): Promise<ExtensionGroupRecord>;
  createExtensionGroup(
    input: ExtensionGroupCreateInput,
  ): Promise<ExtensionGroupRecord>;
  updateExtensionGroup(
    input: ExtensionGroupUpdateInput,
  ): Promise<ExtensionGroupRecord>;
  deleteExtensionGroup(
    input: ExtensionGroupDeleteInput,
  ): Promise<ExtensionGroupRecord>;
  setProfileExtensionGroup(
    input: ProfileExtensionGroupSetInput,
  ): Promise<ProfileRecord>;
  listGroupShares(input: GroupIdInput): Promise<GroupShareGrantList>;
  createGroupShare(input: GroupShareCreateInput): Promise<GroupShareGrant>;
  revokeGroupShare(input: GroupShareRevokeInput): Promise<GroupShareGrant>;
  listMembers(workspaceId: string): Promise<Page<Member>>;
  getMember(input: MemberIdInput): Promise<Member>;
  inviteMember(input: MemberInviteInput): Promise<Member>;
  createMemberInvitation(
    input: MemberInvitationCreateInput,
  ): Promise<MemberInvitationCreateReceipt>;
  redeemMemberInvitation(input: MemberInvitationRedeemInput): Promise<Member>;
  revokeMemberInvitation(
    input: MemberInvitationIdInput,
  ): Promise<MemberInvitationRevocationReceipt>;
  activateMember(input: MemberIdInput): Promise<Member>;
  suspendMember(input: MemberIdInput): Promise<Member>;
  removeMember(input: MemberRemoveInput): Promise<MemberRemoveReceipt>;
  updateMemberRole(input: MemberRoleUpdateInput): Promise<Member>;
  replaceProfileGroups(
    input: ProfileGroupsReplaceInput,
  ): Promise<ProfileRecord>;
  updateProfile(input: ProfileUpdateInput): Promise<ProfileRecord>;
  setProfileProxyAssignment(
    input: ProfileProxyAssignmentSetInput,
  ): Promise<ProfileProxyAssignment>;
  createProfile(input: ProfileCreateInput): Promise<Profile>;
  cloneProfile(input: ProfileCloneInput): Promise<ProfileCloneReceipt>;
  shareProfile(input: ProfileShareInput): Promise<ProfileShareReceipt>;
  listProfileShareRecipientDeviceKeys(
    input: ProfileShareRecipientDeviceKeysInput,
  ): Promise<DeviceEncryptionKey[]>;
  listProfileKeyRotationRecipients(
    profileId: string,
  ): Promise<ProfileKeyRotationRecipientDevicesList>;
  executeProfileKeyRotation(
    input: ProfileKeyRotationExecuteInput,
  ): Promise<OperationAccepted>;
  revokeProfileShare(
    input: ProfileShareRevokeInput,
  ): Promise<ProfileShareRevocationReceipt>;
  archiveProfile(profileId: string, revision: number): Promise<Profile>;
  restoreProfile(profileId: string, revision: number): Promise<Profile>;
  parseProxy(text: string): Promise<ProxyParseReport>;
  importProxyText(input: ProxyImportInput): Promise<ProxyImportReport>;
  checkProxy(input: ProxyCheckInput): Promise<ProxyCheckResult>;
  listProxies(workspaceId: string): Promise<ProxyPage>;
  getProxy(workspaceId: string, proxyId: string): Promise<ProxyRecord>;
  createProxy(input: ProxyCreateInput): Promise<ProxyCreateResult>;
  updateProxy(input: ProxyUpdateInput): Promise<ProxyRecord>;
  deleteProxy(input: ProxyDeleteInput): Promise<ProxyRecord>;
  listGrants(
    workspaceId: string,
    resourceId?: string,
    memberId?: string,
  ): Promise<GrantPage>;
  createGrant(grant: ResourceGrant): Promise<ResourceGrant>;
  revokeGrant(grantId: string): Promise<{ grantId: string; revoked: boolean }>;
  launchProfile(profileId: string): Promise<OperationAccepted>;
  stopProfile(profileId: string): Promise<OperationAccepted>;
  getProfileSyncRecoveryCode(profileId: string): Promise<string>;
  recoverProfileSync(profileId: string, recoveryCode: string): Promise<void>;
  listCookies(
    workspaceId: string,
    profileId: string,
  ): Promise<CookieListResult>;
  stageCookies(input: CookieStageInput): Promise<CookieStageReceipt>;
  importCookies(input: CookieImportInput): Promise<CookieMutationResult>;
  copyCookies(input: CookieCopyInput): Promise<CookieMutationResult>;
  getOperation(operationId: string): Promise<Operation>;
  inspectMigration(sourceRoot: string): Promise<OperationAccepted>;
  importMigration(
    input: Record<string, string | boolean>,
  ): Promise<OperationAccepted>;
  importLegacyProfileData(
    input: MigrationProfileDataImportInput,
  ): Promise<OperationAccepted>;
}

// The Tauri adapter accepts a narrow legacy patch shape and resolves it to a
// revision-checked canonical workspace update over REST. Keep the generated
// API DTO strict; this compatibility input exists only at the desktop bridge.
type WorkspaceUpdateBridgeInput = Omit<WorkspaceUpdateInput, "update"> & {
  update:
    | WorkspaceUpdateInput["update"]
    | {
        name?: string;
        policy?: WorkspaceUpdateInput["update"]["workspace"]["policy"];
      };
};

const invokeNames = {
  systemHealthGet: contractInvoke("system.health.get"),
  eventsStream: contractInvoke("events.stream"),
  accountGet: contractInvoke("account.get"),
  accountIdentityOwnGet: contractInvoke("account.identity.own.get"),
  accountIdentityGet: contractInvoke("account.identity.get"),
  accountIdentityUpdate: contractInvoke("account.identity.update"),
  accountPasswordChange: contractInvoke("account.password.change"),
  accountSessionsList: contractInvoke("account.sessions.list"),
  accountSessionsRevoke: contractInvoke("account.sessions.revoke"),
  accountDraftsGenerate: contractInvoke("management.account.drafts.generate"),
  managementBatch: contractInvoke("management.batch.execute"),
  accountList: contractInvoke("account.list"),
  accountCreate: contractInvoke("account.create"),
  accountCreateScoped: contractInvoke("account.create.scoped"),
  accountStatusUpdate: contractInvoke("account.status.update"),
  authStatus: contractInvoke("desktop.auth.status"),
  authLogout: contractInvoke("desktop.auth.logout"),
  organizationList: contractInvoke("organization.list"),
  organizationGet: contractInvoke("organization.get"),
  organizationRecordGet: contractInvoke("organization.record.get"),
  organizationMembersList: contractInvoke("organization.members.list"),
  organizationAccountCreate: contractInvoke("organization.account.create"),
  organizationMemberStatusUpdate: contractInvoke(
    "organization.member.status.update",
  ),
  organizationMemberWorkspaceAssign: contractInvoke(
    "organization.member.workspace.assign",
  ),
  organizationCreate: contractInvoke("organization.create"),
  organizationUpdate: contractInvoke("organization.update"),
  organizationDelete: contractInvoke("organization.delete"),
  organizationOwnerTransfer: contractInvoke("organization.owner.transfer"),
  workspaceList: contractInvoke("workspace.list"),
  workspaceGet: contractInvoke("workspace.get"),
  workspaceRecordGet: contractInvoke("workspace.record.get"),
  workspaceCreate: contractInvoke("workspace.create"),
  workspaceUpdate: contractInvoke("workspace.update"),
  workspaceDelete: contractInvoke("workspace.delete"),
  teamList: contractInvoke("team.list"),
  teamGet: contractInvoke("team.get"),
  teamCreate: contractInvoke("team.create"),
  teamUpdate: contractInvoke("team.update"),
  resourceList: contractInvoke("resource.list"),
  resourceGet: contractInvoke("resource.get"),
  resourceRegister: contractInvoke("resource.register"),
  groupList: contractInvoke("group.list"),
  groupGet: contractInvoke("group.get"),
  groupCreate: contractInvoke("group.create"),
  groupUpdate: contractInvoke("group.update"),
  groupDelete: contractInvoke("group.delete"),
  extensionGroupList: contractInvoke("extension.group.list"),
  extensionGroupGet: contractInvoke("extension.group.get"),
  extensionGroupCreate: contractInvoke("extension.group.create"),
  extensionGroupUpdate: contractInvoke("extension.group.update"),
  extensionGroupDelete: contractInvoke("extension.group.delete"),
  profileExtensionGroupSet: contractInvoke("profile.extension.group.set"),
  groupShareList: contractInvoke("group.share.list"),
  groupShareCreate: contractInvoke("group.share.create"),
  groupShareRevoke: contractInvoke("group.share.revoke"),
  memberList: contractInvoke("member.list"),
  memberGet: contractInvoke("member.get"),
  memberInvite: contractInvoke("member.invite"),
  memberInvitationCreate: contractInvoke("member.invitation.create"),
  memberInvitationRedeem: contractInvoke("member.invitation.redeem"),
  memberInvitationRevoke: contractInvoke("member.invitation.revoke"),
  memberActivate: contractInvoke("member.activate"),
  memberSuspend: contractInvoke("member.suspend"),
  memberRemove: contractInvoke("member.remove"),
  memberRoleUpdate: contractInvoke("member.role.update"),
  profileGroupsReplace: contractInvoke("profile.groups.replace"),
  profileUpdate: contractInvoke("profile.update"),
  profileProxyAssignmentSet: contractInvoke("profile.proxy.assignment.set"),
  profileList: contractInvoke("profile.list"),
  profileGet: contractInvoke("profile.get"),
  profileDelete: contractInvoke("profile.delete"),
  profileBindingGet: contractInvoke("profile.binding.get"),
  profileProxyAssignmentGet: contractInvoke("profile.proxy.assignment.get"),
  profileCreate: contractInvoke("profile.create"),
  profileClone: contractInvoke("profile.clone"),
  profileShare: contractInvoke("profile.share"),
  profileShareRecipientDeviceKeysList: contractInvoke(
    "profile.share.recipientdevicekeys.list",
  ),
  profileKeyRotationRecipientsList: contractInvoke(
    "profile.keyrotation.recipients.list",
  ),
  profileKeyRotationExecute: contractInvoke("profile.keyrotation.execute"),
  profileShareRevoke: contractInvoke("profile.share.revoke"),
  profileArchive: contractInvoke("profile.archive"),
  profileRestore: contractInvoke("profile.restore"),
  profileLaunch: contractInvoke("profile.launch"),
  profileStop: contractInvoke("profile.stop"),
  profileSyncRecoveryCodeGet: contractInvoke("profile.sync.recovery.code.get"),
  profileSyncRecover: contractInvoke("profile.sync.recover"),
  cookieList: contractInvoke("cookie.list"),
  cookieStage: contractInvoke("cookie.stage"),
  cookieImport: contractInvoke("cookie.import"),
  cookieCopy: contractInvoke("cookie.copy"),
  proxyParse: contractInvoke("proxy.parse"),
  proxyImport: contractInvoke("proxy.import"),
  proxyCheck: contractInvoke("proxy.check"),
  proxyList: contractInvoke("proxy.list"),
  proxyGet: contractInvoke("proxy.get"),
  proxyCreate: contractInvoke("proxy.create"),
  proxyUpdate: contractInvoke("proxy.update"),
  proxyDelete: contractInvoke("proxy.delete"),
  grantList: contractInvoke("grant.list.page"),
  grantCreate: contractInvoke("grant.create"),
  grantRevoke: contractInvoke("grant.revoke"),
  operationGet: contractInvoke("operation.get"),
  migrationInspect: contractInvoke("migration.inspect"),
  migrationImport: contractInvoke("migration.import"),
  migrationProfileDataImport: contractInvoke("migration.profile.data.import"),
} as const;

export function createTauriDesktopApi(
  nativeInvoke: Invoke = tauriInvoke,
): DesktopApi {
  const invoke: Invoke = async <Result>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<Result> => {
    try {
      const result = await nativeInvoke<Result>(command, args);
      if (command === "desktop_auth_login") authInvalidationDispatched = false;
      return result;
    } catch (error) {
      const problem = normalizeProblem(error);
      if (
        !authInvalidationDispatched &&
        [
          "authentication_required",
          "invalid_session",
          "session_expired",
          "session_revoked",
        ].includes(problem.code)
      ) {
        authInvalidationDispatched = true;
        if (typeof window !== "undefined") {
          window.dispatchEvent(new Event(DESKTOP_AUTH_STATUS_CHANGED_EVENT));
        }
      }
      throw error;
    }
  };
  const call = async <Result>(
    command: string,
    payload: Record<string, unknown>,
    idempotent = false,
    idempotencyKeyOverride?: string,
  ) => {
    const correlationId = createRequestId("cor");
    // Retry the same mutation envelope so the API can replay its durable receipt.
    const idempotencyKey = idempotent
      ? (idempotencyKeyOverride ?? createRequestId("idem"))
      : undefined;
    const maxAttempts = idempotent ? 2 : 1;
    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      try {
        const result = await invoke<CommandResult<Result>>(command, {
          correlationId,
          payload,
          ...(idempotencyKey ? { idempotencyKey } : {}),
        });
        return result.data;
      } catch (error) {
        const problem = normalizeProblem(error, correlationId);
        if (!idempotent || !problem.retryable || attempt + 1 >= maxAttempts) {
          throw problem;
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    throw new Error("Unreachable desktop command retry state");
  };
  const callAuth = async (
    command: string,
    correlation = true,
    idempotent = false,
  ) => {
    const correlationId = createRequestId("cor");
    const idempotencyKey = idempotent ? createRequestId("idem") : undefined;
    for (let attempt = 0; attempt < (idempotent ? 2 : 1); attempt += 1) {
      try {
        return await invoke<DesktopAuthStatus>(command, {
          ...(correlation ? { correlationId } : {}),
          ...(idempotencyKey ? { idempotencyKey } : {}),
        });
      } catch (error) {
        const problem = normalizeProblem(error, correlationId);
        if (!idempotent || !problem.retryable || attempt > 0) throw problem;
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    throw new Error("Unreachable auth command retry state");
  };
  return {
    getSystemHealth: () => call(invokeNames.systemHealthGet, {}),
    getIntegrationStatus: async () => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<DesktopIntegrationStatus>(
          "integrations_status_get",
          {
            correlationId,
          },
        );
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    listApiTokens: async () => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<ApiTokenPage>("api_tokens_list", { correlationId });
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    createApiToken: async (input) => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<{ token: string; tokenRecord: ApiToken }>(
          "api_token_create",
          { correlationId, ...input },
        );
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    revokeApiToken: async (tokenId) => {
      const correlationId = createRequestId("cor");
      try {
        await invoke<void>("api_token_revoke", { correlationId, tokenId });
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    subscribeEvents: async (input, onEvent, onError) => {
      const unlistenEvent = await tauriListen<DesktopStreamEvent>(
        "buglogin://api-event",
        ({ payload }) => {
          if (payload.workspaceId === input.workspaceId) onEvent(payload);
        },
      );
      const unlistenError = await tauriListen<DesktopStreamError>(
        "buglogin://api-event-error",
        ({ payload }) => {
          if (payload.workspaceId === input.workspaceId) onError?.(payload);
        },
      );
      let receipt: EventSubscriptionReceipt;
      try {
        receipt = await call<EventSubscriptionReceipt>(
          invokeNames.eventsStream,
          input,
        );
      } catch (error) {
        unlistenEvent();
        unlistenError();
        throw error;
      }
      return async () => {
        unlistenEvent();
        unlistenError();
        await invoke("events_stream_stop", {
          subscriptionId: receipt.subscriptionId,
        });
      };
    },
    getAccount: () => call<AccountView>(invokeNames.accountGet, {}),
    getOwnAccountIdentity: () =>
      call<AccountIdentity>(invokeNames.accountIdentityOwnGet, {}),
    getAccountIdentity: (input) =>
      call<AccountIdentity>(invokeNames.accountIdentityGet, input),
    updateOwnAccountIdentity: (input) =>
      call<AccountIdentity>(invokeNames.accountIdentityUpdate, input, true),
    changeOwnAccountPassword: (input) =>
      call<AccountPasswordChangeReceipt>(
        invokeNames.accountPasswordChange,
        input,
        true,
      ),
    listAccountSessions: (accountId) =>
      call<
        import("../../../frontend/src/generated/command-registry.ts").WebSessionPage
      >(invokeNames.accountSessionsList, { accountId }),
    revokeAccountSession: (accountId, sessionId) =>
      call<AccountSessionRevokeReceipt>(
        invokeNames.accountSessionsRevoke,
        { accountId, sessionId },
        true,
      ),
    generateAccountDrafts: (input) =>
      call<AccountDraftPage>(invokeNames.accountDraftsGenerate, input),
    managementBatch: (input, key) => {
      const { idempotencyKey, ...body } = input;
      return call<ManagementBatchReceipt>(
        invokeNames.managementBatch,
        body,
        true,
        key ?? idempotencyKey,
      );
    },
    listAccounts: () =>
      call<Page<AccountAdminRecord>>(invokeNames.accountList, {}),
    createPendingAccount: (input, idempotencyKey) =>
      call<AccountProvisioningReceipt>(
        invokeNames.accountCreate,
        input,
        true,
        idempotencyKey,
      ),
    createScopedPendingAccount: (input, idempotencyKey) =>
      call<AccountProvisioningReceipt>(
        invokeNames.accountCreateScoped,
        input,
        true,
        idempotencyKey,
      ),
    updateAccountStatus: (input) =>
      call<Account>(invokeNames.accountStatusUpdate, input, true),
    authStatus: () => callAuth(invokeNames.authStatus, false),
    login: async (identifier, password) => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<DesktopAuthStatus>("desktop_auth_login", {
          identifier,
          password,
          correlationId,
        });
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    startBugLabSso: async () => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<number>("desktop_auth_sso_start", {
          correlationId,
        });
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    completeBugLabSso: async (callbackState, token) => {
      const correlationId = createRequestId("cor");
      try {
        const status = await invoke<DesktopAuthStatus>(
          "desktop_auth_sso_complete",
          { callbackState, token, correlationId },
        );
        authInvalidationDispatched = false;
        return status;
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    currentDeepLinks: async () => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<string[]>("desktop_deep_link_current", {});
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    getRuntimeCatalog: async () => {
      const correlationId = createRequestId("cor");
      try {
        return await invoke<RuntimeCatalog>("runtime_catalog_get", {
          correlationId,
        });
      } catch (error) {
        throw normalizeProblem(error, correlationId);
      }
    },
    logout: () => callAuth(invokeNames.authLogout, true, true),
    listOrganizations: () =>
      call<Page<Organization>>(invokeNames.organizationList, {}),
    getOrganization: (input) =>
      call<Organization>(invokeNames.organizationGet, input),
    getOrganizationRecord: (input) =>
      call<OrganizationRecord>(invokeNames.organizationRecordGet, input),
    listOrganizationMembers: (organizationId) =>
      call<OrganizationMemberPage>(invokeNames.organizationMembersList, {
        organizationId,
      }),
    createOrganizationAccount: (input, idempotencyKey) =>
      call<AccountProvisioningReceipt>(
        invokeNames.organizationAccountCreate,
        input,
        true,
        idempotencyKey,
      ),
    updateOrganizationMemberStatus: (input) =>
      call<OrganizationMember>(
        invokeNames.organizationMemberStatusUpdate,
        input,
        true,
      ),
    assignOrganizationMemberToWorkspace: (input, idempotencyKey) =>
      call<Member>(
        invokeNames.organizationMemberWorkspaceAssign,
        input,
        true,
        idempotencyKey,
      ),
    createOrganization: (input) =>
      call<Organization>(invokeNames.organizationCreate, input, true),
    updateOrganization: (input) =>
      call<Organization>(invokeNames.organizationUpdate, input, true),
    deleteOrganization: (input) =>
      call<OrganizationDeleteReceipt>(
        invokeNames.organizationDelete,
        input,
        true,
      ),
    transferOrganizationOwner: (input) =>
      call<Organization>(invokeNames.organizationOwnerTransfer, input, true),
    listWorkspaces: (organizationId) =>
      call<Page<Workspace>>(invokeNames.workspaceList, { organizationId }),
    getWorkspace: (input) => call<Workspace>(invokeNames.workspaceGet, input),
    getWorkspaceRecord: (input) =>
      call<WorkspaceRecord>(invokeNames.workspaceRecordGet, input),
    createWorkspace: (input) =>
      call<Workspace>(invokeNames.workspaceCreate, input, true),
    updateWorkspace: (input) =>
      call<Workspace>(invokeNames.workspaceUpdate, input, true),
    deleteWorkspace: (input) =>
      call<WorkspaceDeleteReceipt>(invokeNames.workspaceDelete, input, true),
    listTeams: (workspaceId) =>
      call<TeamRecordList>(invokeNames.teamList, { workspaceId }),
    getTeam: (input) => call<TeamRecord>(invokeNames.teamGet, input),
    createTeam: (input) =>
      call<TeamRecord>(invokeNames.teamCreate, input, true),
    updateTeam: (input) =>
      call<TeamRecord>(invokeNames.teamUpdate, input, true),
    listResources: (input) =>
      call<Page<Resource>>(invokeNames.resourceList, input),
    getResource: (input) => call<Resource>(invokeNames.resourceGet, input),
    registerResource: (input) =>
      call<Resource>(invokeNames.resourceRegister, input, true),
    listGroups: (workspaceId) =>
      call<GroupRecordList>(invokeNames.groupList, { workspaceId }),
    getGroup: (input) => call<GroupRecord>(invokeNames.groupGet, input),
    createGroup: (input) =>
      call<GroupRecord>(invokeNames.groupCreate, input, true),
    updateGroup: (input) =>
      call<GroupRecord>(invokeNames.groupUpdate, input, true),
    deleteGroup: (input) =>
      call<GroupRecord>(invokeNames.groupDelete, input, true),
    listExtensionGroups: (workspaceId) =>
      call<ExtensionGroupRecordList>(invokeNames.extensionGroupList, {
        workspaceId,
      }),
    getExtensionGroup: (input) =>
      call<ExtensionGroupRecord>(invokeNames.extensionGroupGet, input),
    createExtensionGroup: (input) =>
      call<ExtensionGroupRecord>(invokeNames.extensionGroupCreate, input, true),
    updateExtensionGroup: (input) =>
      call<ExtensionGroupRecord>(invokeNames.extensionGroupUpdate, input, true),
    deleteExtensionGroup: (input) =>
      call<ExtensionGroupRecord>(invokeNames.extensionGroupDelete, input, true),
    setProfileExtensionGroup: (input) =>
      call<ProfileRecord>(invokeNames.profileExtensionGroupSet, input, true),
    listGroupShares: (input) =>
      call<GroupShareGrantList>(invokeNames.groupShareList, input),
    createGroupShare: (input) =>
      call<GroupShareGrant>(invokeNames.groupShareCreate, input, true),
    revokeGroupShare: (input) =>
      call<GroupShareGrant>(invokeNames.groupShareRevoke, input, true),
    listMembers: async (workspaceId) => {
      const items: Member[] = [];
      let cursor: string | undefined;
      const seen = new Set<string>();
      for (let pageCount = 0; pageCount < 1000; pageCount += 1) {
        const page = await call<Page<Member>>(invokeNames.memberList, {
          workspaceId,
          limit: 100,
          ...(cursor ? { cursor } : {}),
        });
        items.push(...page.items);
        if (!page.nextCursor) return { items, nextCursor: null };
        if (seen.has(page.nextCursor))
          throw new Error("member_pagination_cursor_cycle");
        seen.add(page.nextCursor);
        cursor = page.nextCursor;
      }
      throw new Error("member_pagination_page_limit_exceeded");
    },
    getMember: (input) => call<Member>(invokeNames.memberGet, input),
    inviteMember: (input) =>
      call<Member>(invokeNames.memberInvite, input, true),
    createMemberInvitation: (input) =>
      call<MemberInvitationCreateReceipt>(
        invokeNames.memberInvitationCreate,
        input,
        true,
      ),
    redeemMemberInvitation: (input) =>
      call<Member>(invokeNames.memberInvitationRedeem, input, true),
    revokeMemberInvitation: (input) =>
      call<MemberInvitationRevocationReceipt>(
        invokeNames.memberInvitationRevoke,
        input,
        true,
      ),
    activateMember: (input) =>
      call<Member>(invokeNames.memberActivate, input, true),
    suspendMember: (input) =>
      call<Member>(invokeNames.memberSuspend, input, true),
    removeMember: (input) =>
      call<MemberRemoveReceipt>(invokeNames.memberRemove, input, true),
    updateMemberRole: (input) =>
      call<Member>(invokeNames.memberRoleUpdate, input, true),
    replaceProfileGroups: (input) =>
      call<ProfileRecord>(invokeNames.profileGroupsReplace, input, true),
    updateProfile: (input) =>
      call<ProfileRecord>(invokeNames.profileUpdate, input, true),
    setProfileProxyAssignment: (input) =>
      call<ProfileProxyAssignment>(
        invokeNames.profileProxyAssignmentSet,
        input,
        true,
      ),
    listProfiles: async (includeArchived = false, page) => {
      if (page) {
        return call<ProfilePage>(invokeNames.profileList, {
          includeArchived,
          ...page,
        });
      }
      const items: ProfilePage["items"] = [];
      let cursor: string | undefined;
      const seenCursors = new Set<string>();
      do {
        const result = await call<ProfilePage>(invokeNames.profileList, {
          includeArchived,
          limit: 100,
          ...(cursor ? { cursor } : {}),
        });
        items.push(...result.items);
        cursor = result.nextCursor ?? undefined;
        if (cursor && (cursor.length > 512 || seenCursors.has(cursor))) {
          throw new Error("Profile list cursor did not advance");
        }
        if (cursor) seenCursors.add(cursor);
      } while (cursor);
      return { items, nextCursor: null };
    },
    getProfileBinding: (profileId) =>
      call<ProfileResource>(invokeNames.profileBindingGet, { profileId }),
    getProfileProxyAssignment: async (input) => {
      try {
        return await call<ProfileProxyAssignment>(
          invokeNames.profileProxyAssignmentGet,
          input,
        );
      } catch (error) {
        if (
          isProblem(error) &&
          error.code === "profile_proxy_assignment_not_found"
        ) {
          return null;
        }
        throw error;
      }
    },
    getProfile: (input) => call<ProfileRecord>(invokeNames.profileGet, input),
    deleteProfile: (input) =>
      call<DeleteReceipt>(invokeNames.profileDelete, input, true),
    createProfile: (input) =>
      call<Profile>(invokeNames.profileCreate, input, true),
    cloneProfile: (input) =>
      call<ProfileCloneReceipt>(invokeNames.profileClone, input, true),
    shareProfile: (input) =>
      call<ProfileShareReceipt>(invokeNames.profileShare, input, true),
    listProfileShareRecipientDeviceKeys: (input) =>
      call<DeviceEncryptionKey[]>(
        invokeNames.profileShareRecipientDeviceKeysList,
        input,
      ),
    listProfileKeyRotationRecipients: (profileId) =>
      call<ProfileKeyRotationRecipientDevicesList>(
        invokeNames.profileKeyRotationRecipientsList,
        { profileId },
      ),
    executeProfileKeyRotation: (input) =>
      call<OperationAccepted>(
        invokeNames.profileKeyRotationExecute,
        input,
        true,
      ),
    revokeProfileShare: (input) =>
      call<ProfileShareRevocationReceipt>(
        invokeNames.profileShareRevoke,
        input,
        true,
      ),
    archiveProfile: (profileId, revision) =>
      call<Profile>(invokeNames.profileArchive, { profileId, revision }, true),
    restoreProfile: (profileId, revision) =>
      call<Profile>(invokeNames.profileRestore, { profileId, revision }, true),
    parseProxy: (text) =>
      call<ProxyParseReport>(invokeNames.proxyParse, { text }),
    importProxyText: (input) =>
      call<ProxyImportReport>(invokeNames.proxyImport, input, true),
    checkProxy: (input) =>
      call<ProxyCheckResult>(invokeNames.proxyCheck, input),
    listProxies: (workspaceId) =>
      call<ProxyPage>(invokeNames.proxyList, { workspaceId }),
    getProxy: (workspaceId, proxyId) =>
      call<ProxyRecord>(invokeNames.proxyGet, { workspaceId, proxyId }),
    createProxy: (input) =>
      call<ProxyCreateResult>(invokeNames.proxyCreate, input, true),
    updateProxy: (input) =>
      call<ProxyRecord>(invokeNames.proxyUpdate, input, true),
    deleteProxy: (input) =>
      call<ProxyRecord>(invokeNames.proxyDelete, input, true),
    listGrants: async (workspaceId, resourceId, memberId) => {
      const items: ResourceGrant[] = [];
      let cursor: string | undefined;
      const seen = new Set<string>();
      for (let pageCount = 0; pageCount < 1000; pageCount += 1) {
        const page = await call<GrantPage>(invokeNames.grantList, {
          workspaceId,
          limit: 100,
          ...(cursor ? { cursor } : {}),
          ...(resourceId ? { resourceId } : {}),
          ...(memberId ? { memberId } : {}),
        });
        items.push(...page.items);
        if (!page.nextCursor) return { items, nextCursor: null };
        if (seen.has(page.nextCursor))
          throw new Error("grant_pagination_cursor_cycle");
        seen.add(page.nextCursor);
        cursor = page.nextCursor;
      }
      throw new Error("grant_pagination_page_limit_exceeded");
    },
    createGrant: (grant) =>
      call<ResourceGrant>(invokeNames.grantCreate, grant, true),
    revokeGrant: (grantId) =>
      call<{ grantId: string; revoked: boolean }>(
        invokeNames.grantRevoke,
        { grantId },
        true,
      ),
    launchProfile: (profileId) =>
      call<OperationAccepted>(
        invokeNames.profileLaunch,
        { profileId, headless: false },
        true,
      ),
    stopProfile: (profileId) =>
      call<OperationAccepted>(invokeNames.profileStop, { profileId }, true),
    getProfileSyncRecoveryCode: async (profileId) =>
      (
        await call<{ recoveryCode: string }>(
          invokeNames.profileSyncRecoveryCodeGet,
          { profileId },
        )
      ).recoveryCode,
    recoverProfileSync: async (profileId, recoveryCode) => {
      await call<{ recovered: boolean }>(
        invokeNames.profileSyncRecover,
        { profileId, recoveryCode },
        true,
      );
    },
    listCookies: (workspaceId, profileId) =>
      call<CookieListResult>(invokeNames.cookieList, {
        workspaceId,
        profileId,
      }),
    stageCookies: (input) =>
      call<CookieStageReceipt>(invokeNames.cookieStage, input, true),
    importCookies: (input) =>
      call<CookieMutationResult>(invokeNames.cookieImport, input, true),
    copyCookies: (input) =>
      call<CookieMutationResult>(invokeNames.cookieCopy, input, true),
    getOperation: (operationId) =>
      call<Operation>(invokeNames.operationGet, { operationId }),
    inspectMigration: (sourceRoot) =>
      call<OperationAccepted>(
        invokeNames.migrationInspect,
        { sourceRoot },
        true,
      ),
    importMigration: (input) =>
      call<OperationAccepted>(invokeNames.migrationImport, input, true),
    importLegacyProfileData: (input) =>
      call<OperationAccepted>(
        invokeNames.migrationProfileDataImport,
        input,
        true,
      ),
  };
}

export function normalizeProblem(
  error: unknown,
  correlationId = createRequestId("cor"),
): Problem {
  if (isProblem(error)) return error;
  const offline =
    error instanceof TypeError ||
    String(error).toLowerCase().includes("network");
  return {
    type: "about:blank",
    title: offline ? "Desktop service unavailable" : "Desktop command failed",
    status: offline ? 503 : 500,
    code: offline ? "desktop_offline" : "desktop_command_failed",
    correlationId,
    detail: offline
      ? "Cannot reach the native desktop service."
      : error instanceof Error
        ? error.message
        : String(error),
    retryable: offline,
  };
}

function contractInvoke(commandId: CommandId) {
  const command = COMMANDS[commandId];
  if (!(command.adapters as readonly string[]).includes("tauri"))
    throw new Error(`${commandId} is not available through Tauri`);
  return commandId.replaceAll(".", "_");
}
function createRequestId(prefix: string) {
  return `${prefix}_${crypto.randomUUID()}`;
}
function isProblem(value: unknown): value is Problem {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<Problem>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.status === "number" &&
    typeof candidate.correlationId === "string"
  );
}

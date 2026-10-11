import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import {
  COMMANDS,
  type CommandId,
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
  ProfilePage,
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
  Workspace,
} from "./contracts.js";

type Invoke = <Result>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<Result>;
export interface DesktopApi {
  authStatus(): Promise<DesktopAuthStatus>;
  startDeviceAuthorization(): Promise<DesktopAuthStatus>;
  pollDeviceAuthorization(): Promise<DesktopAuthStatus>;
  logout(): Promise<DesktopAuthStatus>;
  listProfiles(includeArchived?: boolean): Promise<ProfilePage>;
  listOrganizations(): Promise<Page<Organization>>;
  listWorkspaces(organizationId: string): Promise<Page<Workspace>>;
  createProfile(input: ProfileCreateInput): Promise<Profile>;
  cloneProfile(input: ProfileCloneInput): Promise<ProfileCloneReceipt>;
  shareProfile(input: ProfileShareInput): Promise<ProfileShareReceipt>;
  revokeProfileShare(
    input: ProfileShareRevokeInput,
  ): Promise<ProfileShareRevocationReceipt>;
  archiveProfile(profileId: string, revision: number): Promise<Profile>;
  restoreProfile(profileId: string, revision: number): Promise<Profile>;
  parseProxy(text: string): Promise<ProxyParseReport>;
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
}

const invokeNames = {
  authStatus: contractInvoke("desktop.auth.status"),
  authDeviceStart: contractInvoke("desktop.auth.device.start"),
  authDevicePoll: contractInvoke("desktop.auth.device.poll"),
  authLogout: contractInvoke("desktop.auth.logout"),
  organizationList: contractInvoke("organization.list"),
  workspaceList: contractInvoke("workspace.list"),
  profileList: contractInvoke("profile.list"),
  profileCreate: contractInvoke("profile.create"),
  profileClone: contractInvoke("profile.clone"),
  profileShare: contractInvoke("profile.share"),
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
  proxyList: contractInvoke("proxy.list"),
  proxyGet: contractInvoke("proxy.get"),
  proxyCreate: contractInvoke("proxy.create"),
  proxyUpdate: contractInvoke("proxy.update"),
  proxyDelete: contractInvoke("proxy.delete"),
  grantList: contractInvoke("grant.list"),
  grantCreate: contractInvoke("grant.create"),
  grantRevoke: contractInvoke("grant.revoke"),
  operationGet: contractInvoke("operation.get"),
  migrationInspect: contractInvoke("migration.inspect"),
  migrationImport: contractInvoke("migration.import"),
} as const;

export function createTauriDesktopApi(
  invoke: Invoke = tauriInvoke,
): DesktopApi {
  const call = async <Result>(
    command: string,
    payload: Record<string, unknown>,
    idempotent = false,
  ) => {
    const correlationId = createRequestId("cor");
    try {
      const result = await invoke<CommandResult<Result>>(command, {
        correlationId,
        payload,
        ...(idempotent ? { idempotencyKey: createRequestId("idem") } : {}),
      });
      return result.data;
    } catch (error) {
      throw normalizeProblem(error, correlationId);
    }
  };
  const callAuth = async (command: string, correlation = true) => {
    const correlationId = createRequestId("cor");
    try {
      return await invoke<DesktopAuthStatus>(
        command,
        correlation ? { correlationId } : undefined,
      );
    } catch (error) {
      throw normalizeProblem(error, correlationId);
    }
  };
  return {
    authStatus: () => callAuth(invokeNames.authStatus, false),
    startDeviceAuthorization: () => callAuth(invokeNames.authDeviceStart),
    pollDeviceAuthorization: () => callAuth(invokeNames.authDevicePoll),
    logout: () => callAuth(invokeNames.authLogout),
    listOrganizations: () =>
      call<Page<Organization>>(invokeNames.organizationList, {}),
    listWorkspaces: (organizationId) =>
      call<Page<Workspace>>(invokeNames.workspaceList, { organizationId }),
    listProfiles: (includeArchived = false) =>
      call<ProfilePage>(invokeNames.profileList, { includeArchived }),
    createProfile: (input) =>
      call<Profile>(invokeNames.profileCreate, input, true),
    cloneProfile: (input) =>
      call<ProfileCloneReceipt>(invokeNames.profileClone, input, true),
    shareProfile: (input) =>
      call<ProfileShareReceipt>(invokeNames.profileShare, input, true),
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
    // ponytail: ProxyParseInput has no published field schema; isolate the provisional text field here.
    parseProxy: (text) =>
      call<ProxyParseReport>(invokeNames.proxyParse, { text }),
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
    listGrants: (workspaceId, resourceId, memberId) =>
      call<GrantPage>(invokeNames.grantList, {
        workspaceId,
        ...(resourceId ? { resourceId } : {}),
        ...(memberId ? { memberId } : {}),
      }),
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
      call<OperationAccepted>(invokeNames.profileStop, { id: profileId }, true),
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

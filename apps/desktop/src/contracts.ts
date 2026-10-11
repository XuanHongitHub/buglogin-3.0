export type ProfileRuntime = 'wayfern' | 'bugox' | 'bugium' | 'bugmini' | 'bugthor' | 'bughelium';
export type ProfileState = 'created' | 'ready' | 'launching' | 'running' | 'stopping' | 'failed' | 'recovering';
export type Profile = { id: string; name: string; runtime: ProfileRuntime; runtimeVersion?: string; state: ProfileState; archived?: boolean; revision?: number; updatedAt?: string };
export type ProfilePage = { items: Profile[]; nextCursor?: string | null };
export type ProfileCreateInput = { name: string; runtime: 'wayfern'; runtimeVersion?: string; workspaceId?: string; proxyId?: string };
export type ProfileCloneInput = { sourceProfileId: string; targetProfileId: string; name: string; expectedRevision: number };
export type ProfileCloneReceipt = { sourceProfileId: string; profile: Profile; resourceId: string; metadataOnly: boolean; copiedFields: string[]; excludedFields: string[] };
export type ProfileSharePermission = 'read' | 'operate';
export type ProfileShareInput = { profileId: string; memberId: string; permission: ProfileSharePermission; expectedRevision: number };
export type ProfileShareReceipt = { profileId: string; memberId: string; permission?: ProfileSharePermission; revision: number; shared: boolean };
export type ProfileShareRevokeInput = { profileId: string; memberId: string; permission: ProfileSharePermission; expectedRevision: number };
export type ProfileShareRevocationReceipt = { profileId: string; memberId: string; revision: number; revoked: boolean };
export type Organization = { id: string; name: string };
export type Workspace = { id: string; organizationId: string; name: string };
export type Page<Item> = { items: Item[]; nextCursor?: string | null };
export type OperationAccepted = { operationId: string; commandId: string; status: 'accepted' };
export type OperationStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
export type Operation = { operationId: string; status: OperationStatus; progress?: number; error?: Problem };
export type ProxyParseRow = { line: number; protocol?: string; endpoint?: string; duplicate?: boolean; valid: boolean; error?: string };
export type ProxyParseReport = { validCount?: number; invalidCount?: number; duplicateCount?: number; rows?: ProxyParseRow[]; [key: string]: unknown };export type ProxyProtocol = 'http' | 'https' | 'socks4' | 'socks5';
export type ProxyView = { id: string; name?: string; protocol: ProxyProtocol; host: string; port: number; hasCredential: boolean; rotationConfigured: boolean; bypassRules: string[]; createdAt: string; updatedAt: string };
export type ProxyRecord = { workspaceId: string; proxy: ProxyView; revision: number };
export type ProxyPage = { items: ProxyRecord[] };
export type ProxyMetadataInput = { id: string; name?: string; protocol: ProxyProtocol; host: string; port: number; bypassRules: string[]; createdAt: string; updatedAt: string };
export type ProxyCreateInput = { workspaceId: string; proxy: ProxyMetadataInput };
export type ProxyUpdateInput = { workspaceId: string; proxyId: string; expectedRevision: number; proxy: ProxyMetadataInput };
export type ProxyDeleteInput = { workspaceId: string; proxyId: string; expectedRevision: number };
export type CookieSummary = { name: string; domain: string; path: string; secure: boolean; httpOnly: boolean; expiresAt?: number | null; sameSite: 'unspecified' | 'none' | 'lax' | 'strict'; valuePresent: boolean };
export type CookieListResult = { generation: number; cookies: CookieSummary[] };
export type CookieWireInput = { name: string; domain: string; path: string; secure: boolean; httpOnly: boolean; expiresAt?: number | null; sameSite: CookieSummary['sameSite']; value: string };
export type CookieStageInput = { workspaceId: string; profileId: string; cookies: CookieWireInput[] };
export type CookieStageReceipt = { stageId: string };
export type CookieImportInput = { workspaceId: string; profileId: string; stageId: string; expectedGeneration: number };
export type CookieCopyInput = { sourceWorkspaceId: string; sourceProfileId: string; destinationWorkspaceId: string; destinationProfileId: string; expectedGeneration: number };
export type CookieMutationResult = { report: { inserted: number; replaced: number; deduplicated: number; copied: number; cleanupPending: number }; generation: number; replayed: boolean };
export type ProxyCreateResult = { record: ProxyRecord; deduplicated: boolean };
export type GrantAction = 'read' | 'create' | 'update' | 'delete' | 'execute' | 'manage';
export type GrantEffect = 'allow' | 'deny';
export type ResourceGrant = { id: string; workspaceId: string; resourceId: string; memberId: string; action: GrantAction; effect: GrantEffect };
export type GrantPage = { items: ResourceGrant[]; nextCursor?: string | null };
export type Problem = { type: string; title: string; status: number; code: string; correlationId: string; detail?: string; retryable?: boolean; fields?: Record<string, unknown> };
export type CommandResult<Data> = { commandId: string; correlationId: string; data: Data };
export type DesktopAuthStatus =
  | { state: 'auth_not_applicable' }
  | { state: 'unauthenticated' }
  | { state: 'authorizing'; userCode: string; verificationUri: string; expiresAt: number; interval: number }
  | { state: 'authenticated' };





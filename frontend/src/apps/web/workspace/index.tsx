import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { Button, Field, Panel, Status, Table, type TableColumn, colors, spacing, typography } from '../../../design-system';
import { errorMessage, WebApiError, type MemberRole, type MemberSummary, type MutationOptions, type ProxyPage, type WebApiClient, type WorkspaceOperationalData, type WorkspaceSummary } from '../api';
import { WebShell, pageHeaderStyle, useOnline } from '../shells';
import { ProxyInventory } from './proxies';

const workspaceNav = [{ href: '/workspaces', label: 'Workspaces' }, { href: '/account/profile', label: 'Account' }];
type WorkspaceData = WorkspaceOperationalData | WorkspaceSummary | WorkspaceSummary[] | ProxyPage;
type WorkspaceSection = 'overview' | 'members' | 'access' | 'proxies' | 'audit';
type WorkspaceLoadState = { status: 'loading' | 'offline' | 'forbidden' } | { status: 'empty'; message: string } | { status: 'error'; message: string; correlationId: string } | { status: 'ready'; data: WorkspaceData };

export function WorkspacePage({ api, path, section = 'overview', workspaceId }: { api: WebApiClient; path: string; section?: WorkspaceSection; workspaceId?: string }) {
  const online = useOnline();
  if (section === 'audit' && workspaceId) return <WorkspaceAuditPage path={path} workspaceId={workspaceId} />;
  const load = useCallback((signal: AbortSignal): Promise<WorkspaceData> => {
    if (!workspaceId) return api.reads.workspaces(signal);
    if (section === 'proxies') return api.reads.proxies(workspaceId, signal);
    return section === 'members' || section === 'access' ? api.reads.workspaceOperational(workspaceId, signal) : api.reads.workspace(workspaceId, signal);
  }, [api, section, workspaceId]);
  const { state, retry } = useWorkspaceState(load, online);
  const nav = workspaceId ? [{ href: '/workspaces', label: 'All workspaces' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/overview`, label: 'Overview' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/resources/proxies`, label: 'Proxies' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/members`, label: 'Members' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/access`, label: 'Access' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/audit`, label: 'Activity & audit' }, { href: '/account/profile', label: 'Account' }] : workspaceNav;
  const shellTitle = workspaceId ? `Workspace ${workspaceId}` : 'Workspaces';
  return <WebShell activePath={path} mode="workspace" nav={nav} title={shellTitle}><h1 style={pageHeaderStyle}>{workspaceId ? section === 'members' ? 'Workspace members' : section === 'access' ? 'Workspace access' : section === 'proxies' ? 'Workspace proxies' : 'Workspace overview' : 'Workspaces'}</h1><WorkspaceState path={path} retry={retry} section={section} state={state} workspaceId={workspaceId} api={api} /></WebShell>;
}

function WorkspaceAuditPage({ path, workspaceId }: { path: string; workspaceId: string }) {
  const nav = [{ href: '/workspaces', label: 'All workspaces' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/overview`, label: 'Overview' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/resources/proxies`, label: 'Proxies' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/members`, label: 'Members' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/access`, label: 'Access' }, { href: `/workspaces/${encodeURIComponent(workspaceId)}/audit`, label: 'Activity & audit' }, { href: '/account/profile', label: 'Account' }];
  return <WebShell activePath={path} mode="workspace" nav={nav} title="Workspace"><h1 style={pageHeaderStyle}>Workspace activity &amp; audit</h1><Panel title="Audit data unavailable" description={`Workspace ${workspaceId} is addressable, but the current web API contract exposes no scoped audit-events query. No records are inferred or displayed.`} actions={<Status label="Contract blocker" tone="warning" />}><p style={{ margin: 0 }}>A backend audit query contract is required before this screen can show activity, filters, export, or investigation details.</p></Panel></WebShell>;
}

function WorkspaceState({ path, retry, section, state, workspaceId, api }: { path: string; retry: () => void; section: WorkspaceSection; state: WorkspaceLoadState; workspaceId?: string; api: WebApiClient }) {
  if (state.status === 'loading') return <Panel aria-live="polite" title="Loading workspace data" description="Requesting current workspace scope…" />;
  if (state.status === 'offline') return <Panel title="Workspace data unavailable offline" description="Reconnect to retrieve workspace scope and sync status. Remote actions remain disabled." actions={<Button onClick={retry}>Retry</Button>} />;
  if (state.status === 'forbidden') return <Panel title="Workspace access denied" description="This workspace is outside your permitted scope, or your session lacks the required access." />;
  if (state.status === 'error') return <Panel title="Unable to load workspace data" description={`${state.message} Correlation ID: ${state.correlationId}`} actions={<Button onClick={retry}>Retry</Button>} />;
  if (state.status === 'empty') return <Panel title="No workspaces assigned" description={state.message} />;
  if (state.status !== 'ready') return null;
  if (Array.isArray(state.data)) return <WorkspaceTable rows={state.data} />;
  if (section === 'proxies') return isProxyPage(state.data) && workspaceId ? <ProxyInventory api={api} page={state.data} workspaceId={workspaceId} retry={retry} /> : <Panel title="Proxy data unavailable" description="The workspace proxy response did not include a valid inventory." actions={<Button onClick={retry}>Retry</Button>} />;
  if (section === 'members') return isWorkspaceOperationalData(state.data) ? <MemberDirectory api={api} data={state.data} retry={retry} /> : <Panel title="Member data unavailable" description="The workspace operational response did not include a valid member directory." actions={<Button onClick={retry}>Retry</Button>} />;
  if (section === 'access') return isWorkspaceOperationalData(state.data) ? <AccessDirectory api={api} data={state.data} retry={retry} /> : <Panel title="Access data unavailable" description="The workspace operational response did not include a valid access directory." actions={<Button onClick={retry}>Retry</Button>} />;
  if (!isWorkspaceSummary(state.data)) return <Panel title="Workspace data unavailable" description="The workspace API returned an unsupported response for this view." actions={<Button onClick={retry}>Retry</Button>} />;
  void path;
  void workspaceId;
  return <WorkspaceOverview workspace={state.data} />;
}

function isWorkspaceOperationalData(data: WorkspaceData): data is WorkspaceOperationalData {
  return !Array.isArray(data) && 'workspace' in data;
}

function isProxyPage(data: WorkspaceData): data is ProxyPage {
  return !Array.isArray(data) && 'items' in data && Array.isArray(data.items);
}

function isWorkspaceSummary(data: WorkspaceData): data is WorkspaceSummary {
  return !Array.isArray(data) && 'policy' in data;
}

function WorkspaceTable({ rows }: { rows: WorkspaceSummary[] }) {
  const columns: TableColumn<WorkspaceSummary>[] = [
    { key: 'name', header: 'Workspace', render: workspace => <a href={`/workspaces/${workspace.id}/overview`} style={{ color: colors.brand[700], fontWeight: 650 }}>{workspace.name}</a> },
    { key: 'organization', header: 'Organization', render: workspace => workspace.organizationId },
    { key: 'profiles', header: 'Profile limit', render: workspace => workspace.policy.maxProfiles.toLocaleString() },
    { key: 'members', header: 'Member limit', render: workspace => workspace.policy.maxMembers.toLocaleString() },
    { key: 'offline', header: 'Offline access', render: workspace => <Status label={workspace.policy.offlineEntitled ? 'Entitled' : 'Not entitled'} tone={workspace.policy.offlineEntitled ? 'success' : 'warning'} /> },
  ];
  return <Panel title="Assigned workspaces" description="Workspace access remains server-enforced."><Table caption="Assigned workspaces" columns={columns} emptyMessage="No workspaces are assigned to this account." getRowKey={workspace => workspace.id} rows={rows} /></Panel>;
}

function WorkspaceOverview({ workspace }: { workspace: WorkspaceSummary }) {
  return <div style={{ display: 'grid', gap: spacing[4] }}><Panel actions={<Status label={workspace.policy.offlineEntitled ? 'Offline entitled' : 'Online only'} tone={workspace.policy.offlineEntitled ? 'success' : 'info'} />} description={`Organization ID: ${workspace.organizationId}`} title={workspace.name}><p style={{ margin: 0 }}>Workspace ID: {workspace.id}</p><p style={{ marginBottom: 0 }}>Created at: {workspace.createdAt}</p></Panel><div style={{ display: 'grid', gap: spacing[4], gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))' }}><Panel title="Profile limit" description="Maximum profiles allowed by policy."><strong style={{ fontSize: 26 }}>{workspace.policy.maxProfiles.toLocaleString()}</strong></Panel><Panel title="Member limit" description="Maximum members allowed by policy."><strong style={{ fontSize: 26 }}>{workspace.policy.maxMembers.toLocaleString()}</strong></Panel><Panel title="Storage limit" description="Maximum storage allowed by policy."><strong style={{ fontSize: 26 }}>{workspace.policy.maxStorageBytes.toLocaleString()} bytes</strong></Panel></div><Panel title="Members" description="Review current workspace membership and access status." actions={<a href={`/workspaces/${encodeURIComponent(workspace.id)}/members`} style={{ color: colors.brand[700], fontWeight: 650 }}>Open directory</a>} /></div>;
}

function MemberDirectory({ api, data, retry }: { api: WebApiClient; data: WorkspaceOperationalData; retry: () => void }) {
  const [accountId, setAccountId] = useState('');
  const [role, setRole] = useState<MemberRole>('member');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();
  const activeCount = data.members.filter(member => member.status === 'active').length;
  const columns: TableColumn<MemberSummary>[] = [
    { key: 'account', header: 'Account', render: member => member.accountId },
    { key: 'role', header: 'Role', render: member => member.role },
    { key: 'team', header: 'Team', render: member => member.teamId ?? 'Unassigned' },
    { key: 'status', header: 'Status', render: member => <Status label={member.status} tone={member.status === 'active' ? 'success' : member.status === 'suspended' ? 'danger' : 'warning'} /> },
    { key: 'actions', header: 'Actions', render: member => <span style={{ display: 'inline-flex', gap: spacing[2] }}><Button disabled={saving || member.status === 'active'} onClick={() => void changeMember(member, 'activate')}>Activate</Button><Button disabled={saving || member.status !== 'active'} variant="danger" onClick={() => void changeMember(member, 'suspend')}>Suspend</Button><select aria-label={`Role for ${member.accountId}`} disabled={saving} value={member.role} onChange={event => void changeRole(member, event.currentTarget.value as MemberRole)}><option value="owner">Owner</option><option value="admin">Admin</option><option value="operator">Operator</option><option value="member">Member</option></select></span> },
  ];
  const changeMember = async (member: MemberSummary, action: 'activate' | 'suspend') => {
    setSaving(true); setError(undefined); setMessage(undefined);
    try {
      const options: MutationOptions = { idempotencyKey: `member-${action}-${member.id}`, signal: new AbortController().signal };
      if (action === 'activate') await api.writes.activateMember(data.workspace.id, member.id, options);
      else await api.writes.suspendMember(data.workspace.id, member.id, options);
      setMessage(action === 'activate' ? 'Member activated.' : 'Member suspended.'); retry();
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setSaving(false); }
  };
  const changeRole = async (member: MemberSummary, nextRole: MemberRole) => {
    if (nextRole === member.role) return;
    setSaving(true); setError(undefined); setMessage(undefined);
    try {
      await api.writes.updateMemberRole(data.workspace.id, member.id, { role: nextRole }, { idempotencyKey: `member-role-${member.id}-${nextRole}`, signal: new AbortController().signal });
      setMessage('Member role updated.'); retry();
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setSaving(false); }
  };
  const invite = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!accountId.trim()) return;
    setSaving(true); setError(undefined); setMessage(undefined);
    const member: MemberSummary = { id: `member_${crypto.randomUUID().replaceAll('-', '').slice(0, 24)}`, accountId: accountId.trim(), workspaceId: data.workspace.id, teamId: null, role, status: 'invited', inviteExpiresAt: null };
    try {
      await api.writes.inviteMember(data.workspace.id, member, { idempotencyKey: `member-invite-${member.id}`, signal: new AbortController().signal });
      setAccountId(''); setMessage('Invitation created.'); retry();
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setSaving(false); }
  };
  return <div style={{ display: 'grid', gap: spacing[4] }}><Panel actions={<Status label={`${activeCount}/${data.members.length} active`} tone="info" />} description={`Workspace ID: ${data.workspace.id}. Server-authorized member records.`} title={data.workspace.name}><form onSubmit={invite} style={{ display: 'flex', flexWrap: 'wrap', gap: spacing[2], alignItems: 'end' }}><Field label="Account" required value={accountId} onChange={event => setAccountId(event.currentTarget.value)} placeholder="account ID" /><label style={{ display: 'grid', gap: spacing[1] }}><span style={typography.label}>Role</span><select aria-label="Invite role" value={role} onChange={event => setRole(event.currentTarget.value as MemberRole)}><option value="admin">Admin</option><option value="operator">Operator</option><option value="member">Member</option></select></label><Button type="submit" variant="primary" loading={saving}>Invite member</Button></form>{message ? <p role="status">{message}</p> : null}{error ? <p role="alert">{error}</p> : null}</Panel><Panel title="Members" description="Live membership data from the workspace operational API."><Table caption={`${data.workspace.name} members`} columns={columns} emptyMessage="No members are assigned to this workspace." getRowKey={member => member.id} rows={data.members} /></Panel><Panel title="Refresh state" description="The directory reloads from the API after each mutation." actions={<Button onClick={retry}>Refresh</Button>} /></div>;
}

function AccessDirectory({ api, data, retry }: { api: WebApiClient; data: WorkspaceOperationalData; retry: () => void }) {
  const [memberId, setMemberId] = useState(data.members.find(member => member.status === 'active')?.id ?? '');
  const [resourceId, setResourceId] = useState(data.resources[0]?.id ?? '');
  const [action, setAction] = useState('read');
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
  const canSubmit = Boolean(memberId && resourceId);
  const submit = async () => {
    if (!canSubmit) return;
    setSaving(true); setError(undefined); setMessage(undefined);
    const grantId = `grant_${crypto.randomUUID().replaceAll('-', '').slice(0, 24)}`;
    try {
      await api.writes.createGrant(data.workspace.id, { id: grantId, workspaceId: data.workspace.id, resourceId, memberId, action, effect: 'allow' }, { idempotencyKey: `grant-create-${grantId}`, signal: new AbortController().signal });
      setMessage('Access grant created.');
      retry();
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setSaving(false); }
  };
  const revoke = async (grantId: string) => {
    setSaving(true); setError(undefined); setMessage(undefined);
    try {
      await api.writes.revokeGrant(data.workspace.id, grantId, { idempotencyKey: `grant-revoke-${grantId}`, signal: new AbortController().signal });
      setMessage('Access grant revoked.');
      retry();
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setSaving(false); }
  };
  return <div style={{ display: 'grid', gap: spacing[4] }}>
    <Panel title="Grant access" description="Server-side ACL, audit, tenant checks, and idempotency apply to every mutation.">
      <div style={{ display: 'grid', gap: spacing[3], gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', alignItems: 'end' }}>
        <label><span>Member</span><select value={memberId} onChange={event => setMemberId(event.currentTarget.value)}><option value="">Select member</option>{data.members.filter(member => member.status === 'active').map(member => <option key={member.id} value={member.id}>{member.accountId}</option>)}</select></label>
        <label><span>Resource</span><select value={resourceId} onChange={event => setResourceId(event.currentTarget.value)}><option value="">Select resource</option>{data.resources.map(resource => <option key={resource.id} value={resource.id}>{resource.displayName}</option>)}</select></label>
        <label><span>Permission</span><select value={action} onChange={event => setAction(event.currentTarget.value)}>{['read', 'execute', 'update', 'manage'].map(value => <option key={value} value={value}>{value}</option>)}</select></label>
        <Button disabled={!canSubmit || saving} loading={saving} onClick={() => void submit()}>Grant access</Button>
      </div>
      {message ? <p role="status">{message}</p> : null}{error ? <p role="alert">{error}</p> : null}
    </Panel>
    <Panel title="Current grants" description={`${data.grants.length} server-authorized grants.`}>
      <Table caption="Workspace grants" columns={[{ key: 'member', header: 'Member', render: grant => data.members.find(member => member.id === grant.memberId)?.accountId ?? grant.memberId }, { key: 'resource', header: 'Resource', render: grant => data.resources.find(resource => resource.id === grant.resourceId)?.displayName ?? grant.resourceId }, { key: 'action', header: 'Permission', render: grant => grant.action }, { key: 'effect', header: 'Effect', render: grant => <Status label={grant.effect} tone={grant.effect === 'allow' ? 'success' : 'danger'} /> }, { key: 'revoke', header: 'Action', render: grant => <Button variant="quiet" disabled={saving} onClick={() => void revoke(grant.id)}>Revoke</Button> }]} emptyMessage="No grants are assigned." getRowKey={grant => grant.id} rows={data.grants} />
    </Panel>
    <Panel title="Refresh state" description="Data reloads from the API after every mutation." actions={<Button onClick={retry}>Refresh</Button>} />
  </div>;
}

function useWorkspaceState(load: (signal: AbortSignal) => Promise<WorkspaceData>, online: boolean) {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<WorkspaceLoadState>({ status: 'loading' });
  const retry = useCallback(() => setRevision(value => value + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    load(controller.signal).then(data => setState(Array.isArray(data) && data.length === 0 ? { status: 'empty', message: 'No workspace membership was returned for this account.' } : { status: 'ready', data })).catch(error => {
      if (controller.signal.aborted) return;
      if (!online) setState({ status: 'offline' });
      else if (error instanceof WebApiError && [401, 403].includes(error.problem.status)) setState({ status: 'forbidden' });
      else setState({ status: 'error', message: errorMessage(error), correlationId: error instanceof WebApiError ? error.problem.correlationId : 'unknown' });
    });
    return () => controller.abort();
  }, [load, online, revision]);
  return { retry, state };
}

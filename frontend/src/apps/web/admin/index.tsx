import { useCallback } from 'react';
import { Panel, Status, Table, type TableColumn, spacing } from '../../../design-system';
import type { AdminSummary, MemberSummary, OrganizationSummary, WebApiClient, WorkspaceOperationalData } from '../api';
import { AsyncState, WebShell, pageHeaderStyle, useLoadState } from '../shells';

const adminNav = [
  { href: '/admin/overview', label: 'Overview' },
  { href: '/admin/users', label: 'Users and identities' },
  { href: '/admin/organizations', label: 'Organizations' },
  { href: '/admin/workspaces', label: 'Workspaces' },
  { href: '/admin/incidents', label: 'Incidents' },
  { href: '/admin/audit', label: 'Audit & investigations' },
];

export function AdminPage({ api, path }: { api: WebApiClient; path: string }) {
  const load = useCallback((signal: AbortSignal) => api.reads.adminSummary(signal), [api]);
  if (path === '/admin/incidents' || path === '/admin/audit') return <AdminUnavailablePage path={path} />;
  const { state, retry } = useLoadState(load, () => false, 'No authorized platform data was returned.');
  return <WebShell activePath={path} mode="admin" nav={adminNav} title="Platform admin">
    <h1 style={pageHeaderStyle}>{adminTitle(path)}</h1>
    <AsyncState retry={retry} state={state}>{summary => <AdminRoute path={path} summary={summary} />}</AsyncState>
  </WebShell>;
}

function adminTitle(path: string) {
  return ({
    '/admin': 'Overview',
    '/admin/overview': 'Overview',
    '/admin/users': 'Users and identities',
    '/admin/organizations': 'Organizations',
    '/admin/workspaces': 'Workspaces',
    '/admin/incidents': 'Incidents',
    '/admin/audit': 'Audit & investigations',
  } as Record<string, string>)[path] ?? 'Platform admin';
}

function AdminUnavailablePage({ path }: { path: string }) {
  const title = path === '/admin/incidents' ? 'Incidents' : 'Audit & investigations';
  return <WebShell activePath={path} mode="admin" nav={adminNav} title="Platform admin"><h1 style={pageHeaderStyle}>{title}</h1><Panel title={`${title} unavailable`} description="The current web API contract does not expose this platform-admin query. No incidents, events, counts, or status are fabricated." actions={<Status label="Contract blocker" tone="warning" />}><p style={{ margin: 0 }}>{path === '/admin/incidents' ? 'Incident listing, detail, updates, ownership, and recovery status require an approved incidents API.' : 'Cross-domain audit search, scoped filtering, investigation cases, and export require an approved audit query API.'}</p></Panel></WebShell>;
}

function AdminRoute({ path, summary }: { path: string; summary: AdminSummary }) {
  if (path === '/admin/users') return <UserTable rows={userRows(summary)} />;
  if (path === '/admin/organizations') return <OrganizationTable rows={summary.organizations} />;
  if (path === '/admin/workspaces') return <WorkspaceTable organizations={summary.organizations} rows={summary.workspaces} />;
  return <AdminOverview summary={summary} />;
}

function AdminOverview({ summary }: { summary: AdminSummary }) {
  const memberCount = summary.workspaces.reduce((total, item) => total + item.members.length, 0);
  const resourceCount = summary.workspaces.reduce((total, item) => total + item.resources.length, 0);
  const users = userRows(summary);
  return <div style={{ display: 'grid', gap: spacing[4] }}>
    <Panel actions={<Status label={summary.account.status ?? 'Authorized'} tone="success" />} description={`Account ID: ${summary.account.accountId}. Counts reflect authorized records returned by the current API scope.`} title="Current admin scope" />
    <div style={{ display: 'grid', gap: spacing[4], gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))' }}>
      <Metric label="Organizations" value={summary.organizations.length} />
      <Metric label="Workspaces" value={summary.workspaces.length} />
      <Metric label="Identities" value={users.length} />
      <Metric label="Memberships" value={memberCount} />
      <Metric label="Resources" value={resourceCount} />
    </div>
    <Panel description="Audit events, background jobs, and platform sync health are not exposed by the current web API contract. No status is inferred here." title="Governance telemetry" />
  </div>;
}

function Metric({ label, value }: { label: string; value: number }) {
  return <Panel title={label}><strong style={{ fontSize: 26 }}>{value.toLocaleString()}</strong></Panel>;
}

function OrganizationTable({ rows }: { rows: OrganizationSummary[] }) {
  const columns: TableColumn<OrganizationSummary>[] = [
    { key: 'name', header: 'Organization', render: row => row.name },
    { key: 'id', header: 'ID', render: row => row.id },
    { key: 'account', header: 'Account', render: row => row.accountId },
    { key: 'created', header: 'Created', render: row => formatDate(row.createdAt) },
  ];
  return <Panel title="Authorized organizations" description="Only organizations allowed by server-side grants are shown."><Table caption="Authorized organizations" columns={columns} emptyMessage="No organizations are available." getRowKey={row => row.id} rows={rows} /></Panel>;
}

function WorkspaceTable({ organizations, rows }: { organizations: OrganizationSummary[]; rows: WorkspaceOperationalData[] }) {
  const organizationNames = new Map(organizations.map(organization => [organization.id, organization.name]));
  const columns: TableColumn<WorkspaceOperationalData>[] = [
    { key: 'name', header: 'Workspace', render: row => row.workspace.name },
    { key: 'organization', header: 'Organization', render: row => organizationNames.get(row.workspace.organizationId) ?? row.workspace.organizationId },
    { key: 'members', header: 'Members', render: row => row.members.length.toLocaleString() },
    { key: 'resources', header: 'Resources', render: row => row.resources.length.toLocaleString() },
    { key: 'grants', header: 'Grants', render: row => row.grants.length.toLocaleString() },
    { key: 'policy', header: 'Policy', render: row => `${row.workspace.policy.maxProfiles} profiles · ${row.workspace.policy.maxMembers} members` },
  ];
  return <Panel title="Authorized workspaces" description="Operational counts come from live workspace APIs."><Table caption="Authorized workspaces" columns={columns} emptyMessage="No workspaces are available." getRowKey={row => row.workspace.id} rows={rows} /></Panel>;
}

type UserRow = {
  accountId: string;
  memberships: number;
  workspaces: number;
  statuses: string;
  roles: string;
};

function userRows(summary: AdminSummary): UserRow[] {
  const members = summary.workspaces.flatMap(item => item.members);
  const accounts = new Map<string, MemberSummary[]>();
  accounts.set(summary.account.accountId, []);
  for (const member of members) accounts.set(member.accountId, [...(accounts.get(member.accountId) ?? []), member]);
  return [...accounts.entries()].map(([accountId, records]) => ({
    accountId,
    memberships: records.length,
    workspaces: new Set(records.map(record => record.workspaceId)).size,
    statuses: summarizeValues(records.map(record => record.status)),
    roles: summarizeValues(records.map(record => record.role)),
  })).sort((left, right) => left.accountId.localeCompare(right.accountId));
}

function UserTable({ rows }: { rows: UserRow[] }) {
  const columns: TableColumn<UserRow>[] = [
    { key: 'account', header: 'Account ID', render: row => row.accountId },
    { key: 'memberships', header: 'Memberships', render: row => row.memberships.toLocaleString(), align: 'right' },
    { key: 'workspaces', header: 'Workspaces', render: row => row.workspaces.toLocaleString(), align: 'right' },
    { key: 'status', header: 'Status', render: row => row.statuses },
    { key: 'roles', header: 'Roles', render: row => row.roles },
  ];
  return <Panel title="Users and identities" description="Identity rows are derived from the current account and authorized workspace memberships. No platform-wide user total is assumed."><Table caption="Users and identities" columns={columns} emptyMessage="No identities are available in the current scope." getRowKey={row => row.accountId} rows={rows} /></Panel>;
}

function summarizeValues(values: string[]): string {
  return values.length === 0 ? 'Current account' : [...new Set(values)].sort().join(', ');
}

function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleDateString();
}

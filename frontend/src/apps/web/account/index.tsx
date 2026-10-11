import { useCallback, useEffect, useState } from 'react';
import { Button, Field, Panel, Status, colors, spacing, typography } from '../../../design-system';
import { errorMessage, WebApiError, type AccountSummary, type ApiTokenSummary, type WebApiClient, type WebSessionSummary } from '../api';
import { WebShell, pageHeaderStyle, useOnline } from '../shells';

const accountNav = [{ href: '/account/profile', label: 'Profile' }, { href: '/account/security', label: 'Security' }];
type AccountLoadState = { status: 'loading' | 'offline' | 'forbidden' | 'empty' } | { status: 'error'; message: string; correlationId: string } | { status: 'ready'; data: AccountSummary };

export function AccountPage({ api, path }: { api: WebApiClient; path: string }) {
  const online = useOnline();
  const load = useCallback((signal: AbortSignal) => api.reads.account(signal), [api]);
  const { state, retry } = useAccountState(load, online);
  return <WebShell activePath={path} mode="account" nav={accountNav} title="Account"><h1 style={pageHeaderStyle}>{accountTitle(path)}</h1><AccountState api={api} path={path} retry={retry} state={state} /></WebShell>;
}

function accountTitle(path: string) {
  void path;
  return 'Account profile';
}

function AccountState({ api, path, retry, state }: { api: WebApiClient; path: string; retry: () => void; state: AccountLoadState }) {
  if (state.status === 'loading') return <Panel aria-live="polite" title={`Loading ${accountTitle(path).toLowerCase()}`} description="Requesting current account data…" />;
  if (state.status === 'offline') return <Panel title="Account data unavailable offline" description="Reconnect to retrieve the latest protected account data. Local navigation remains available." actions={<Button onClick={retry}>Retry</Button>} />;
  if (state.status === 'forbidden') return <Panel title="Account access denied" description="Your session cannot read this account area. No protected account data was returned." />;
  if (state.status === 'error') return <Panel title={`Unable to load ${accountTitle(path).toLowerCase()}`} description={`${state.message} Correlation ID: ${state.correlationId}`} actions={<Button onClick={retry}>Retry</Button>} />;
  if (state.status === 'empty') return <Panel title="Account profile is empty" description="The account service returned no profile record. Retry after confirming the session." actions={<Button onClick={retry}>Retry</Button>} />;
  if (state.status !== 'ready') return null;
  return <AccountRoute account={state.data} api={api} path={path} />;
}

function AccountRoute({ account, api, path }: { account: AccountSummary; api: WebApiClient; path: string }) {
  void path;
  const logout = async () => {
    await api.auth.logout(new AbortController().signal);
    navigateToLogin();
  };
  return <div style={{ display: 'grid', gap: spacing[4] }}>
    <Panel actions={<Button onClick={logout}>Sign out</Button>} title="Account" description="Authenticated account identifier returned by the account API."><p style={{ margin: 0 }}>Account ID: {account.accountId}</p></Panel>
    <SessionInventory api={api} />
    {path === '/account/security' ? <ApiTokenInventory api={api} /> : null}
  </div>;
}

type SessionState = { status: 'loading' } | { status: 'error'; message: string } | { status: 'ready'; sessions: WebSessionSummary[] };

function SessionInventory({ api }: { api: WebApiClient }) {
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [state, setState] = useState<SessionState>({ status: 'loading' });
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    api.auth.sessions(controller.signal).then(sessions => setState({ status: 'ready', sessions })).catch(error => {
      if (!controller.signal.aborted) setState({ status: 'error', message: errorMessage(error) });
    });
    return () => controller.abort();
  }, [api, revision]);
  const revoke = async (session: WebSessionSummary) => {
    setBusy(session.id);
    try {
      await api.auth.revokeSession(session.id, new AbortController().signal);
      if (session.current) navigateToLogin();
      else setRevision(value => value + 1);
    } finally {
      setBusy(null);
    }
  };
  if (state.status === 'loading') return <Panel aria-live="polite" title="Sessions and devices" description="Loading active account sessions…" />;
  if (state.status === 'error') return <Panel actions={<Button onClick={() => setRevision(value => value + 1)}>Retry</Button>} title="Sessions unavailable" description={state.message} />;
  return <Panel title="Sessions and devices" description="Revoke browsers, desktop devices, or API sessions owned by this account.">
    <div style={{ display: 'grid' }}>{state.sessions.map(session => <div key={session.id} style={{ alignItems: 'center', borderBottom: `1px solid ${colors.border.subtle}`, display: 'grid', gap: spacing[3], gridTemplateColumns: 'minmax(0, 1fr) auto', padding: `${spacing[3]}px 0` }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ alignItems: 'center', display: 'flex', flexWrap: 'wrap', gap: spacing[2] }}><strong style={typography.bodySmall}>{session.deviceId ?? session.kind}</strong>{session.current ? <Status label="Current" tone="success" /> : null}{session.revokedAt ? <Status label="Revoked" tone="neutral" /> : null}</div>
        <div style={{ color: colors.text.muted, marginTop: spacing[1], ...typography.caption }}>Expires {formatTime(session.expiresAt)} · ID {session.id.slice(0, 12)}</div>
      </div>
      <Button disabled={session.revokedAt !== null} loading={busy === session.id} onClick={() => revoke(session)} variant={session.current ? 'danger' : 'secondary'}>{session.current ? 'Revoke current' : 'Revoke'}</Button>
    </div>)}</div>
  </Panel>;
}

function formatTime(epochSeconds: number) {
  return new Date(epochSeconds * 1000).toLocaleString();
}

function navigateToLogin() {
  if (typeof window === 'undefined') return;
  window.history.replaceState(null, '', '/login');
  window.dispatchEvent(new PopStateEvent('popstate'));
}

function useAccountState(load: (signal: AbortSignal) => Promise<AccountSummary>, online: boolean) {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<AccountLoadState>({ status: 'loading' });
  const retry = useCallback(() => setRevision(value => value + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    load(controller.signal).then(data => setState(data ? { status: 'ready', data } : { status: 'empty' })).catch(error => {
      if (controller.signal.aborted) return;
      if (!online) setState({ status: 'offline' });
      else if (error instanceof WebApiError && [401, 403].includes(error.problem.status)) setState({ status: 'forbidden' });
      else setState({ status: 'error', message: errorMessage(error), correlationId: error instanceof WebApiError ? error.problem.correlationId : 'unknown' });
    });
    return () => controller.abort();
  }, [load, online, revision]);
  return { retry, state };
}


type ApiTokenState = { status: 'loading' } | { status: 'error'; message: string } | { status: 'ready'; tokens: ApiTokenSummary[] };

function ApiTokenInventory({ api }: { api: WebApiClient }) {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<ApiTokenState>({ status: 'loading' });
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState('profile.read:read');
  const [createdToken, setCreatedToken] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: 'loading' });
    api.auth.apiTokens(controller.signal).then(tokens => setState({ status: 'ready', tokens })).catch(error => {
      if (!controller.signal.aborted) setState({ status: 'error', message: errorMessage(error) });
    });
    return () => controller.abort();
  }, [api, revision]);
  const create = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy('create');
    try {
      const result = await api.auth.createApiToken({ name: name.trim(), scopes: scopes.split(',').map(value => value.trim()).filter(Boolean), expiresAt: Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60 }, new AbortController().signal);
      setCreatedToken(result.token);
      setName('');
      setRevision(value => value + 1);
    } catch (error) {
      setState({ status: 'error', message: errorMessage(error) });
    } finally {
      setBusy(null);
    }
  };
  const revoke = async (token: ApiTokenSummary) => {
    setBusy(token.id);
    try {
      await api.auth.revokeApiToken(token.id, new AbortController().signal);
      setRevision(value => value + 1);
    } catch (error) {
      setState({ status: 'error', message: errorMessage(error) });
    } finally {
      setBusy(null);
    }
  };
  return <Panel title="API tokens" description="Scoped automation credentials. Raw token appears once. Store it in your vault; BugLogin never displays it again.">
    <form onSubmit={create} style={{ display: 'grid', gap: spacing[3], marginBottom: spacing[4] }}>
      <Field label="Token name" onChange={event => setName(event.target.value)} required value={name} />
      <Field help="Comma-separated capability:action scopes. Example: profile.read:read" label="Scopes" onChange={event => setScopes(event.target.value)} required value={scopes} />
      <Button disabled={!name.trim() || !scopes.trim()} loading={busy === 'create'} type="submit" variant="primary">Create token</Button>
    </form>
    {createdToken ? <div><Status label="Copy this token now. It will not be shown again." tone="warning" live /><code style={{ background: colors.surface.sunken, display: 'block', marginTop: spacing[2], overflowWrap: 'anywhere', padding: spacing[2] }}>{createdToken}</code></div> : null}
    {state.status === 'loading' ? <p style={{ ...typography.bodySmall, color: colors.text.muted }}>Loading token inventory…</p> : null}
    {state.status === 'error' ? <Status label={state.message} tone="danger" live /> : null}
    {state.status === 'ready' ? <div style={{ display: 'grid' }}>{state.tokens.map(token => <div key={token.id} style={{ alignItems: 'center', borderBottom: `1px solid ${colors.border.subtle}`, display: 'grid', gap: spacing[3], gridTemplateColumns: 'minmax(0, 1fr) auto', padding: `${spacing[3]}px 0` }}><div style={{ minWidth: 0 }}><strong style={typography.bodySmall}>{token.name}</strong><div style={{ color: colors.text.muted, marginTop: spacing[1], ...typography.caption }}>{token.scopes.join(', ')} · Expires {formatTime(token.expiresAt)}</div></div><Button disabled={token.revokedAt !== null} loading={busy === token.id} onClick={() => revoke(token)} variant="danger">Revoke</Button></div>)}</div> : null}
  </Panel>;
}

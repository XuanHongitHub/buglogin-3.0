import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { Button, Field, Panel, Status, colors, spacing, typography } from '../../../design-system';
import { errorMessage, WebApiError, type WebApiClient } from '../api';
import { WebShell } from '../shells';

export function LoginPage({ api, path }: { api: WebApiClient; path: string }) {
  const [identifier, setIdentifier] = useState('');
  const [password, setPassword] = useState('');
  const [state, setState] = useState<'idle' | 'loading' | 'error'>('idle');
  const [message, setMessage] = useState('');
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setState('loading');
    setMessage('');
    try {
      await api.auth.login(identifier, password, new AbortController().signal);
      setPassword('');
      if (typeof window !== 'undefined') {
        window.history.replaceState(null, '', '/account/profile');
        window.dispatchEvent(new PopStateEvent('popstate'));
      }
    } catch (error) {
      setState('error');
      setMessage(error instanceof WebApiError && error.problem.code === 'invalid_credentials' ? 'Credentials could not be verified.' : errorMessage(error));
    }
  };
  return <WebShell activePath={path} mode="auth" nav={[{ href: '/', label: 'Back to public site' }]} title="Sign in">
    <div style={{ margin: '0 auto', maxWidth: 440 }}>
      <Panel title="Sign in to BugLogin" description="Hosted access. Browser session stays in Secure HttpOnly cookies.">
        <form onSubmit={submit} style={{ display: 'grid', gap: spacing[4] }}>
          <Field autoComplete="username" label="Username or email" onChange={event => setIdentifier(event.target.value)} required value={identifier} />
          <Field autoComplete="current-password" label="Password" onChange={event => setPassword(event.target.value)} required type="password" value={password} />
          {state === 'error' ? <Status label={message} tone="danger" live /> : null}
          <Button disabled={!identifier.trim() || password.length < 12} loading={state === 'loading'} type="submit" variant="primary">Sign in</Button>
        </form>
      </Panel>
      <p style={{ color: colors.text.muted, margin: `${spacing[3]}px 0 0`, ...typography.caption }}><a href="/recover" style={{ color: colors.brand[700] }}>Recover access</a> · Never share passwords through MCP, URLs, screenshots, or support logs.</p>
    </div>
  </WebShell>;
}

export function WebAuthGate({ api, children, path }: { api: WebApiClient; children: ReactNode; path: string }) {
  const [state, setState] = useState<'loading' | 'authenticated' | 'unauthenticated' | 'error'>('loading');
  useEffect(() => {
    const controller = new AbortController();
    api.auth.session(controller.signal).then(() => setState('authenticated')).catch(error => {
      if (controller.signal.aborted) return;
      setState(error instanceof WebApiError && error.problem.status === 401 ? 'unauthenticated' : 'error');
    });
    return () => controller.abort();
  }, [api]);
  if (state === 'authenticated') return <>{children}</>;
  if (state === 'unauthenticated') return <LoginPage api={api} path="/login" />;
  if (state === 'error') return <WebShell activePath={path} mode="auth" nav={[]} title="Authentication"><Panel title="Authentication unavailable" description="The hosted identity service did not respond safely. Retry from the browser." /></WebShell>;
  return <WebShell activePath={path} mode="auth" nav={[]} title="Authentication"><Panel aria-live="polite" title="Checking session" description="Requesting protected session state…" /></WebShell>;
}

export function DeviceApprovalPage({ api, path }: { api: WebApiClient; path: string }) {
  const [userCode, setUserCode] = useState('');
  const [state, setState] = useState<'idle' | 'loading' | 'success' | 'error'>('idle');
  const [message, setMessage] = useState('');
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setState('loading');
    setMessage('');
    try {
      await api.auth.approveDevice(userCode.trim(), new AbortController().signal);
      setState('success');
      setUserCode('');
    } catch (error) {
      setState('error');
      setMessage(errorMessage(error));
    }
  };
  return <WebShell activePath={path} mode="auth" nav={[{ href: '/account/profile', label: 'Account' }]} title="Device authorization">
    <div style={{ maxWidth: 520 }}>
      <Panel title="Connect BugLogin Desktop" description="Enter the code shown by the desktop app. Codes are single-use and expire automatically.">
        <form onSubmit={submit} style={{ display: 'grid', gap: spacing[4] }}>
          <Field autoCapitalize="characters" autoComplete="one-time-code" label="Device code" onChange={event => setUserCode(event.target.value.toUpperCase())} required value={userCode} />
          {state === 'success' ? <Status label="Desktop device approved" tone="success" live /> : null}
          {state === 'error' ? <Status label={message} tone="danger" live /> : null}
          <Button disabled={!userCode.trim()} loading={state === 'loading'} type="submit" variant="primary">Approve device</Button>
        </form>
      </Panel>
    </div>
  </WebShell>;
}


export function RecoveryPage({ api, path }: { api: WebApiClient; path: string }) {
  const [identifier, setIdentifier] = useState('');
  const [token, setToken] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [requestState, setRequestState] = useState<'idle' | 'loading' | 'success' | 'error'>('idle');
  const [completeState, setCompleteState] = useState<'idle' | 'loading' | 'success' | 'error'>('idle');
  const [message, setMessage] = useState('');
  const requestRecovery = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setRequestState('loading');
    setMessage('');
    try {
      await api.auth.requestRecovery(identifier, new AbortController().signal);
      setRequestState('success');
    } catch (error) {
      setRequestState('error');
      setMessage(errorMessage(error));
    }
  };
  const completeRecovery = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setCompleteState('loading');
    setMessage('');
    try {
      const key = `recovery-complete-${token.slice(0, 16)}-${globalThis.crypto.randomUUID()}`;
      await api.auth.completeRecovery(token.trim(), newPassword, key, new AbortController().signal);
      setToken('');
      setNewPassword('');
      setCompleteState('success');
    } catch (error) {
      setCompleteState('error');
      setMessage(errorMessage(error));
    }
  };
  return <WebShell activePath={path} mode="auth" nav={[{ href: '/login', label: 'Sign in' }]} title="Account recovery">
    <div style={{ display: 'grid', gap: spacing[4], margin: '0 auto', maxWidth: 560 }}>
      <Panel title="Request recovery" description="The response is identical whether the account exists. Delivery uses the configured trusted provider.">
        <form onSubmit={requestRecovery} style={{ display: 'grid', gap: spacing[4] }}>
          <Field autoComplete="username" label="Username or email" onChange={event => setIdentifier(event.target.value)} required value={identifier} />
          {requestState === 'success' ? <Status label="If the account is eligible, recovery instructions were sent." tone="success" live /> : null}
          {requestState === 'error' ? <Status label={message} tone="danger" live /> : null}
          <Button disabled={!identifier.trim()} loading={requestState === 'loading'} type="submit" variant="primary">Request recovery</Button>
        </form>
      </Panel>
      <Panel title="Set a new password" description="Use the single-use token from the recovery message. Completing recovery revokes existing sessions.">
        <form onSubmit={completeRecovery} style={{ display: 'grid', gap: spacing[4] }}>
          <Field autoComplete="one-time-code" label="Recovery token" onChange={event => setToken(event.target.value)} pattern="[0-9a-fA-F]{64}" required value={token} />
          <Field autoComplete="new-password" label="New password" minLength={12} onChange={event => setNewPassword(event.target.value)} required type="password" value={newPassword} />
          {completeState === 'success' ? <Status label="Password updated. Sign in again on every device." tone="success" live /> : null}
          {completeState === 'error' ? <Status label={message} tone="danger" live /> : null}
          <Button disabled={token.trim().length !== 64 || newPassword.length < 12} loading={completeState === 'loading'} type="submit" variant="primary">Reset password</Button>
        </form>
      </Panel>
    </div>
  </WebShell>;
}

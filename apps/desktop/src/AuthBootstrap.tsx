import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Button, Status } from '../../../frontend/src/design-system/index.js';
import { normalizeProblem, type DesktopApi } from './bridge.js';
import type { DesktopAuthStatus, Problem } from './contracts.js';
import './desktop.css';

export function AuthBootstrap({ api, children }: { api: DesktopApi; children: ReactNode }) {
  const [status, setStatus] = useState<DesktopAuthStatus>();
  const [problem, setProblem] = useState<Problem>();
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setProblem(undefined);
      setStatus(await api.authStatus());
    } catch (error) {
      setProblem(normalizeProblem(error));
    }
  }, [api]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (status?.state !== 'authorizing') return;
    const delay = Math.max(status.interval, 1) * 1_000;
    const timer = window.setTimeout(async () => {
      try {
        setStatus(await api.pollDeviceAuthorization());
      } catch (error) {
        setProblem(normalizeProblem(error));
      }
    }, delay);
    return () => window.clearTimeout(timer);
  }, [api, status]);

  if (status?.state === 'authenticated' || status?.state === 'auth_not_applicable') return children;

  const start = async () => {
    setBusy(true);
    try {
      setProblem(undefined);
      setStatus(await api.startDeviceAuthorization());
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setBusy(false);
    }
  };

  return <div className="auth-shell">
    <header className="auth-topbar">
      <div className="window-controls" aria-hidden="true"><span className="window-dot window-dot-red" /><span className="window-dot window-dot-yellow" /><span className="window-dot window-dot-green" /></div>
      <strong>BugLogin</strong>
      <Status label="Production" tone="neutral" />
    </header>
    <main className="auth-content">
      <section className="auth-panel" aria-labelledby="auth-title">
        <div className="eyebrow">Hosted account</div>
        <h1 id="auth-title">Connect this desktop</h1>
        <p>Authorize this device in the BugLogin web app. Account passwords never enter the desktop process.</p>
        {status?.state === 'authorizing' ? <div className="auth-code-block">
          <span>One-time code</span>
          <strong className="mono">{status.userCode}</strong>
          <small>Open <span className="mono">{status.verificationUri}</span>, enter this code, then keep this window open.</small>
          <div className="auth-actions"><Button onClick={() => void navigator.clipboard.writeText(status.userCode)}>Copy code</Button><Button onClick={() => void api.pollDeviceAuthorization().then(setStatus).catch(error => setProblem(normalizeProblem(error)))}>Check now</Button></div>
        </div> : <div className="auth-actions"><Button variant="primary" loading={busy} loadingLabel="Starting…" onClick={() => void start()}>Connect account</Button><Button onClick={() => void load()}>Retry status</Button></div>}
        {problem ? <div className="inline-error" role="alert"><strong>{problem.title}</strong><span>{problem.detail ?? problem.code}</span></div> : null}
      </section>
    </main>
  </div>;
}

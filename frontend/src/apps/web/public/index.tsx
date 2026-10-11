import { useCallback } from 'react';
import { Button, Panel, Status } from '../../../design-system';
import type { LoadState, PublicSummary, WebApiClient } from '../api';
import { WebShell, pageHeaderStyle, useLoadState } from '../shells';

const neverEmpty = () => false;
const publicNav = [{ href: '/', label: 'Home' }, { href: '/download', label: 'Download' }, { href: '/security', label: 'Security' }, { href: '/docs', label: 'Docs' }, { href: '/status', label: 'Status' }];

export function PublicPage({ api, path }: { api: WebApiClient; path: string }) {
  const load = useCallback((signal: AbortSignal) => api.reads.publicSummary(signal), [api]);
  const needsSummary = path === '/' || path.startsWith('/download') || path.startsWith('/status');
  const { state, retry } = useLoadState(load, neverEmpty, 'Public information is unavailable.', needsSummary);
  const summary = state.status === 'ready' ? state.data : undefined;
  const page = path.startsWith('/download')
    ? <DownloadPage summary={summary} state={state} retry={retry} />
    : path.startsWith('/security')
      ? <SecurityPage />
      : path.startsWith('/docs')
        ? <DocumentationPage />
        : path.startsWith('/status')
          ? <StatusPage summary={summary} state={state} retry={retry} />
          : <HomePage summary={summary} state={state} retry={retry} />;

  return <WebShell activePath={path} mode="public" nav={publicNav} title="Product">{page}</WebShell>;
}

function HomePage({ summary, state, retry }: { summary?: PublicSummary; state: LoadState<PublicSummary>; retry: () => void }) {
  return <>
    <section className="buglogin-public-hero" aria-labelledby="public-title">
      <div className="buglogin-public-hero-copy">
        <p className="buglogin-public-eyebrow">Browser operations, under your control</p>
        <h1 id="public-title">Run profiles with clear control over access, runtime, and recovery.</h1>
        <p className="buglogin-public-lede">BugLogin brings desktop profile management and workspace governance into one auditable system. Rust owns protected operations; the interface reports the authority’s actual state.</p>
        <div className="buglogin-public-actions">
          {summary?.release ? <a className="buglogin-public-primary" href={summary.release.downloadUrl}>Download {summary.release.version}</a> : <a className="buglogin-public-primary" href="/download">View release availability</a>}
          <a className="buglogin-public-secondary" href="/security">Read the security model</a>
        </div>
      </div>
      <aside className="buglogin-public-principles" aria-label="Product operating model">
        <div><span>Desktop authority</span><p>Profile files, runtime processes, and local secrets stay under Rust-owned controls.</p></div>
        <div><span>Workspace governance</span><p>Roles, grants, and scope are enforced by the backend and recorded for review.</p></div>
        <div><span>Recoverable operations</span><p>Sync and migration expose partial, blocked, and recoverable states explicitly.</p></div>
      </aside>
    </section>
    <section className="buglogin-public-capabilities" aria-labelledby="capabilities-title">
      <div className="buglogin-public-section-heading"><p className="buglogin-public-eyebrow">Built for daily operations</p><h2 id="capabilities-title">Every action has an owner and a state.</h2></div>
      <div className="buglogin-public-capability-list">
        <article><span>01</span><div><h3>Profiles and runtimes</h3><p>Create, inspect, and launch browser profiles through the desktop authority.</p></div><a href="/docs">Explore the model</a></article>
        <article><span>02</span><div><h3>Access and workspaces</h3><p>Manage members and resource grants with server-enforced scope and audit.</p></div><a href="/security">Review controls</a></article>
        <article><span>03</span><div><h3>Sync and migration</h3><p>See freshness, conflicts, and eligibility before a change reaches production data.</p></div><a href="/docs">Read the workflow</a></article>
      </div>
    </section>
    <section className="buglogin-public-status" aria-labelledby="service-title">
      <div><p className="buglogin-public-eyebrow">Live platform information</p><h2 id="service-title">Service status</h2></div>
      <LiveStatus summary={summary} state={state} retry={retry} />
    </section>
  </>;
}

function DownloadPage({ summary, state, retry }: { summary?: PublicSummary; state: LoadState<PublicSummary>; retry: () => void }) {
  return <section className="buglogin-public-route" aria-labelledby="download-title">
    <p className="buglogin-public-eyebrow">Verified releases</p>
    <h1 id="download-title">Downloads</h1>
    <p className="buglogin-public-lede">Only a published release from the platform catalog is offered here.</p>
    {summary?.release ? <Panel title={`BugLogin ${summary.release.version}`} description="Published in the verified release catalog." actions={<a className="buglogin-public-primary" href={summary.release.downloadUrl}>Download verified release</a>} /> : null}
    {!summary ? <LiveStatus summary={undefined} state={state} retry={retry} /> : null}
    {summary && !summary.release ? <Panel title="No verified release is available" description="The release catalog has not published a downloadable build. Check back after a release is approved." actions={<a href="/status">View platform status</a>} /> : null}
  </section>;
}

function SecurityPage() {
  return <section className="buglogin-public-route" aria-labelledby="security-title">
    <p className="buglogin-public-eyebrow">Trust boundaries</p>
    <h1 id="security-title">Security and authority</h1>
    <p className="buglogin-public-lede">Security controls follow the boundary that owns each operation. The interface presents state; Rust and the hosted API enforce it.</p>
    <div className="buglogin-public-policy-list">
      <article><h2>Desktop operations</h2><p>Rust owns filesystem access, browser processes, local storage, permissions, and state transitions.</p></article>
      <article><h2>Hosted access</h2><p>API requests enforce identity, role, workspace scope, and capability checks on the server.</p></article>
      <article><h2>Secrets and migration</h2><p>Secrets stay in protected storage. Migration preserves source data and requires explicit verification and rollback evidence before cutover.</p></article>
    </div>
    <p className="buglogin-public-note">Production migration and release remain gated until the required platform and rollback evidence is complete.</p>
  </section>;
}

function DocumentationPage() {
  return <section className="buglogin-public-route" aria-labelledby="docs-title">
    <p className="buglogin-public-eyebrow">Product guides</p>
    <h1 id="docs-title">Documentation</h1>
    <p className="buglogin-public-lede">Start with the product areas that match the task you need to complete.</p>
    <div className="buglogin-public-policy-list">
      <article><h2>Profiles and runtimes</h2><p>Understand profile lifecycle, runtime selection, and local launch state.</p><a href="/download">Check release availability</a></article>
      <article><h2>Workspace access</h2><p>Review roles, membership, resource grants, and server-enforced scope.</p><a href="/security">Read the security model</a></article>
      <article><h2>Migration and recovery</h2><p>Review inspection, snapshots, eligibility, rehearsal, and rollback requirements.</p><a href="/security">Review safety boundaries</a></article>
    </div>
  </section>;
}

function StatusPage({ summary, state, retry }: { summary?: PublicSummary; state: LoadState<PublicSummary>; retry: () => void }) {
  return <section className="buglogin-public-route" aria-labelledby="status-title">
    <p className="buglogin-public-eyebrow">Live platform information</p>
    <h1 id="status-title">Service status</h1>
    <p className="buglogin-public-lede">Status is read from the platform health and release endpoints.</p>
    <LiveStatus summary={summary} state={state} retry={retry} />
  </section>;
}

function LiveStatus({ summary, state, retry }: { summary?: PublicSummary; state: LoadState<PublicSummary>; retry: () => void }) {
  if (summary) return <Panel className="buglogin-public-status-panel" title="Current platform state" description={summary.status.detail} actions={<Status label={summary.status.label} tone={summary.status.label.toLowerCase().includes('operational') ? 'success' : 'warning'} />} />;
  if (state.status === 'loading') return <Panel className="buglogin-public-status-panel" aria-live="polite" title="Checking service status" description="Requesting the current status from the platform." />;
  if (state.status === 'error') return <Panel className="buglogin-public-status-panel" title="Live status is unavailable" description={state.message} actions={<Button onClick={retry}>Retry</Button>} />;
  return <Panel className="buglogin-public-status-panel" title="No verified release information" description={state.status === 'empty' ? state.message : 'No release information is available.'} actions={<a href="/download">Check release availability</a>} />;
}

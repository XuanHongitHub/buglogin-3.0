import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(import.meta.dirname, '..', '..', '..');
const read = (path) => readFile(resolve(repoRoot, path), 'utf8');

const [desktop, desktopCss, webShell, workspace, routeMatch, baseline] = await Promise.all([
  read('apps/desktop/src/ProfileWorkspace.tsx'),
  read('apps/desktop/src/desktop.css'),
  read('frontend/src/apps/web/shells/index.tsx'),
  read('frontend/src/apps/web/workspace/index.tsx'),
  read('frontend/src/apps/web/route-match.ts'),
  read('docs/architecture/upstream-baseline-manifest.yaml'),
]);

test('WP-48 UI authority and state contract is represented by real production code', async (suite) => {
  await suite.test('desktop retains the Donut-compatible operations shell', () => {
    assert.match(desktop, /className="desktop-rail"/);
    assert.match(desktop, /aria-label="Application navigation"/);
    assert.match(desktop, /aria-label="Profile groups"/);
    assert.match(desktop, /aria-label="Profile inventory"/);
    assert.match(desktop, /className="profile-table"/);
    assert.match(desktop, /function Drawer\(/);
    assert.match(desktop, /querySelector<HTMLElement>/);
    assert.match(desktop, /event\.key === 'Escape'/);
    assert.match(desktopCss, /\.rail-button:focus-visible/);
    assert.match(desktopCss, /\.profile-search:focus-within/);
    assert.match(desktopCss, /@media\(/);
    assert.doesNotMatch(desktopCss, /(?:linear|radial)-gradient\(/i);
  });

  await suite.test('desktop exposes real loading, offline, error, and action-result states', () => {
    assert.match(desktop, /kind === 'loading'/);
    assert.match(desktop, /kind === 'offline'/);
    assert.match(desktop, /role="alert"/);
    assert.match(desktop, /role="status"/);
    assert.match(desktop, /Waiting for authority/);
    assert.match(desktop, /Authority confirmed/);
  });

  await suite.test('web shell and workspace expose state boundaries without fake counts', () => {
    assert.match(webShell, /Offline\. Read data may be stale/);
    assert.match(webShell, /title="Loading"/);
    assert.match(webShell, /title="Unable to load"/);
    assert.match(webShell, /title="Nothing here yet"/);
    assert.match(workspace, /status === 'forbidden'/);
    assert.match(workspace, /status === 'offline'/);
    assert.match(workspace, /status === 'error'/);
    assert.match(workspace, /Server-authorized/);
    assert.doesNotMatch(workspace, /fake|stub|simulat/i);
  });

  await suite.test('web route surface matches the approved IA entrypoints', () => {
    for (const path of ['/login', '/recover', '/account', '/operations', '/migration', '/workspaces']) {
      assert.match(routeMatch, new RegExp(`path === '${path.replace('/', '\\/')}'`));
    }
    assert.match(routeMatch, /\['\/admin'/);
    assert.match(routeMatch, /resources\/proxies/);
    assert.match(routeMatch, /workspaceId/);
  });

  await suite.test('desktop visual baseline remains pinned to the inspected Donut source', async () => {
    const commit = baseline.match(/commit: ([0-9a-f]{40})/)?.[1];
    assert.equal(commit, 'a4ed5c855afc4242b6c89c7d73ad767f2357c7f0');
    const { execFile } = await import('node:child_process');
    const { promisify } = await import('node:util');
    const run = promisify(execFile);
    const { stdout } = await run('git', ['-C', resolve(repoRoot, '.tmp/donutbrowser-upstream'), 'rev-parse', 'HEAD'], { encoding: 'utf8' });
    assert.equal(stdout.trim(), commit);
  });
});

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const repoRoot = resolve(import.meta.dirname, '../..');
const legacyRoot = process.env.BUGLOGIN_LEGACY_ROOT ?? 'E:\\bug-login';
const snapshotRoot = resolve(repoRoot, '.tmp/buglogin-legacy');
const matrix = readFileSync(resolve(repoRoot, 'docs/migration/source-version-matrix.md'), 'utf8');
const expectedCommit = matrix.match(/Legacy BugLogin `v[^`]+`, commit `([0-9a-f]{40})`/)?.[1];

function git(root, args) {
  return execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
}

function trackedFiles(root) {
  return git(root, ['ls-files']).split(/\r?\n/).filter(Boolean);
}

function sourceFiles(files, prefix) {
  return files.filter(file => file.startsWith(prefix) && /\.(rs|ts|tsx|js|mjs|json)$/.test(file));
}

function commandNames(root, files) {
  const names = [];
  for (const file of files.filter(item => item.startsWith('src-tauri/src/') && item.endsWith('.rs'))) {
    const text = git(root, ['show', `HEAD:${file}`]);
    const lines = text.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      if (!lines[index].includes('#[tauri::command]')) continue;
      const body = lines.slice(index + 1, index + 8).join('\n');
      const match = body.match(/(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)/);
      if (match) names.push({ file, name: match[1] });
    }
  }
  return names.sort((left, right) => left.name.localeCompare(right.name));
}

function routeFiles(files) {
  return files.filter(file => /^src\/app\/.*\/(page|route|layout)\.(ts|tsx)$/.test(file)).sort();
}

function areaCounts(files) {
  const areas = {
    profile: /profile|browser_runner|browser_adapter|chromium_runtime/i,
    proxy: /proxy|vpn|network/i,
    cookies: /cookie/i,
    extensions: /extension/i,
    sharing: /share|permission|grant|lock/i,
    sync: /sync|backup|restore/i,
    control: /admin|workspace|team|member|resource|organization|control|hr/i,
    runtime: /browser_version|runtime|downloader|wayfern|bugox|bugium|bugmini|cloak|thorium|helium/i,
    auth: /auth|login|session|entitlement/i,
  };
  return Object.fromEntries(Object.entries(areas).map(([area, pattern]) => [area, files.filter(file => pattern.test(file)).length]));
}

function inventory(root) {
  const files = trackedFiles(root);
  return {
    commit: git(root, ['rev-parse', 'HEAD']),
    tree: git(root, ['rev-parse', 'HEAD^{tree}']),
    trackedFileCount: files.length,
    sourceFileCount: sourceFiles(files, 'src/').length + sourceFiles(files, 'src-tauri/').length + sourceFiles(files, 'buglogin-sync/').length,
    areaCounts: areaCounts(files),
    tauriCommands: commandNames(root, files),
    webRoutes: routeFiles(files),
  };
}

if (!existsSync(legacyRoot) || !existsSync(snapshotRoot)) {
  console.error(JSON.stringify({ code: 'legacy_audit_source_missing', legacyRoot, snapshotRoot }));
  process.exit(1);
}

const legacy = inventory(legacyRoot);
const snapshot = inventory(snapshotRoot);
const report = {
  schema: 'buglogin.legacy-audit/1',
  generatedAt: new Date().toISOString(),
  source: { label: 'E:/bug-login', readOnly: true },
  snapshot: { label: '.tmp/buglogin-legacy', readOnly: true },
  expectedCommit,
  sourceCommitMatch: legacy.commit === expectedCommit,
  snapshotCommitMatch: snapshot.commit === expectedCommit,
  sourceSnapshotMatch: legacy.commit === snapshot.commit && legacy.tree === snapshot.tree,
  legacy,
  snapshot,
  manualReviewRequired: [
    'Map every legacy command/function to a BugLogin 2.0 capability disposition.',
    'Verify runtime, cookie, extension, proxy, sharing, sync, and control-plane behavior on real data.',
    'Do not treat file or command-name counts as feature parity evidence.',
  ],
};

const output = process.env.BUGLOGIN_LEGACY_AUDIT_OUTPUT;
if (output) writeFileSync(resolve(repoRoot, output), `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
if (!report.sourceCommitMatch || !report.snapshotCommitMatch || !report.sourceSnapshotMatch) process.exitCode = 2;

import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { extname, relative, resolve, sep } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(import.meta.dirname, '..', '..');
const evidenceRoot = resolve(repoRoot, 'artifacts/e2e');
const ignored = new Set(['.gitkeep', 'README.md']);

async function filesUnder(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) files.push(...await filesUnder(path));
    else if (!ignored.has(entry.name)) files.push(path);
  }
  return files;
}

test('WP-48 visual evidence is never treated as complete without real artifacts', async (t) => {
  const files = await filesUnder(evidenceRoot);
  if (process.env.BUGLOGIN_VISUAL_REQUIRE_EVIDENCE !== '1') {
    t.skip('Set BUGLOGIN_VISUAL_REQUIRE_EVIDENCE=1 on an RC evidence run. No screenshot or result is fabricated.');
    return;
  }

  const reports = files.filter((path) => extname(path).toLowerCase() === '.json');
  assert.ok(reports.length > 0, `No result JSON found under ${relative(repoRoot, evidenceRoot)}`);

  for (const reportPath of reports) {
    const report = JSON.parse(await readFile(reportPath, 'utf8'));
    for (const field of ['caseId', 'result', 'artifactSha256', 'versions', 'os', 'architecture', 'fixture', 'startedAt', 'endedAt', 'assertions', 'screenshots']) {
      assert.ok(field in report, `${relative(repoRoot, reportPath)} missing ${field}`);
    }
    assert.match(report.result, /^(passed|failed|blocked)$/);
    assert.ok(Array.isArray(report.assertions));
    assert.ok(Array.isArray(report.screenshots));
    for (const screenshot of report.screenshots) {
      assert.equal(typeof screenshot, 'string');
      const target = resolve(evidenceRoot, screenshot);
      assert.equal(relative(evidenceRoot, target).startsWith(`..${sep}`), false, `${screenshot} escapes evidence root`);
      assert.ok(files.includes(target), `${screenshot} referenced by ${report.caseId} does not exist`);
    }
  }
});

import { useState } from 'react';
import { Button, Field, Panel, Status, colors, spacing, typography } from '../../../design-system';
import { errorMessage, type MigrationImportInput, type MigrationInspectReport, type MigrationInspectResult, type MigrationSourceSnapshot, type Operation, type WebApiClient } from '../api';
import { WebShell, pageHeaderStyle, useOnline } from '../shells';

export function MigrationPage({ api, path }: { api: WebApiClient; path: string }) {
  const online = useOnline();
  const [sourceRoot, setSourceRoot] = useState('');
  const [stagingParent, setStagingParent] = useState('');
  const [destinationRoot, setDestinationRoot] = useState('');
  const [runId, setRunId] = useState(() => `migration-${crypto.randomUUID()}`);
  const [confirmation, setConfirmation] = useState('');
  const [report, setReport] = useState<MigrationInspectResult>();
  const [operation, setOperation] = useState<Operation>();
  const [busy, setBusy] = useState<'inspect' | 'import' | undefined>();
  const [error, setError] = useState<string>();
  const inspect = async () => {
    if (!online || !sourceRoot.trim()) return;
    setBusy('inspect'); setError(undefined); setOperation(undefined); setReport(undefined);
    try {
      const accepted = await api.writes.inspectMigration({ sourceRoot: sourceRoot.trim() }, { idempotencyKey: `migration-inspect-${crypto.randomUUID()}`, signal: new AbortController().signal });
      const completed = await waitForOperation(api, accepted.operationId, setOperation);
      const result = parseInspectResult(completed.result);
      if (!result) throw new Error('Migration inspect returned an unsupported result.');
      setReport(result);
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setBusy(undefined); }
  };
  const importPackage = async () => {
    if (!online || !report || report.report.cutoverEligibility !== 'eligible' || !stagingParent.trim() || !destinationRoot.trim() || !runId.trim() || confirmation !== 'CUTOVER_AND_PROMOTE') return;
    setBusy('import'); setError(undefined); setOperation(undefined);
    try {
      const accepted = await api.writes.importMigration({ sourceRoot: sourceRoot.trim(), stagingParent: stagingParent.trim(), destinationRoot: destinationRoot.trim(), runId: runId.trim(), confirmation }, { idempotencyKey: `migration-import-${runId.trim()}`, signal: new AbortController().signal });
      await waitForOperation(api, accepted.operationId, setOperation);
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setBusy(undefined); }
  };
  const eligible = report?.report.cutoverEligibility === 'eligible';
  return <WebShell activePath={path} mode="operations" nav={[{ href: '/operations', label: 'Profiles and operations' }, { href: '/migration', label: 'Migration' }, { href: '/account/security', label: 'Account security' }]} title="Migration">
    <h1 style={pageHeaderStyle}>Legacy migration</h1>
    <div style={{ display: 'grid', gap: spacing[4] }}>
      <Panel title="Local authority only" description="This workflow sends a filesystem path to a local-authority API. Hosted production mode rejects local migration and never reads the desktop filesystem."><Status label={online ? 'Connected' : 'Offline'} tone={online ? 'info' : 'warning'} /></Panel>
      <Panel title="1. Inspect source" description="Read-only snapshot. The source must remain unchanged; no import is attempted during inspection.">
        <div style={{ display: 'grid', gap: spacing[3] }}><Field label="Legacy source root" required value={sourceRoot} onChange={event => setSourceRoot(event.currentTarget.value)} placeholder="C:\\Users\\...\\BugLogin" help="Use the prepared copy for production rehearsal; do not point at a live source during cutover." /><Button variant="primary" disabled={!online || !sourceRoot.trim()} loading={busy === 'inspect'} onClick={() => void inspect()}>Inspect source</Button></div>
      </Panel>
      {report ? <InspectSummary report={report.report} snapshot={report.snapshot} /> : null}
      <Panel title="2. Import after eligibility" description="Import remains unavailable until inspection reports eligible. The confirmation phrase is intentionally exact.">
        <div style={{ display: 'grid', gap: spacing[3] }}><Field label="Staging parent" value={stagingParent} onChange={event => setStagingParent(event.currentTarget.value)} placeholder="D:\\BugLogin-Migration-Staging" /><Field label="Destination repository" value={destinationRoot} onChange={event => setDestinationRoot(event.currentTarget.value)} placeholder="D:\\BugLogin-2" /><Field label="Run ID" value={runId} onChange={event => setRunId(event.currentTarget.value)} /><Field label="Confirmation" value={confirmation} onChange={event => setConfirmation(event.currentTarget.value)} placeholder="CUTOVER_AND_PROMOTE" help="Do not enter this until snapshot, dry-run, backup, and rollback rehearsal are approved." /><Button variant="danger" disabled={!online || !eligible || !stagingParent.trim() || !destinationRoot.trim() || !runId.trim() || confirmation !== 'CUTOVER_AND_PROMOTE'} loading={busy === 'import'} onClick={() => void importPackage()}>Start import</Button></div>
        {!eligible && report ? <p style={{ color: colors.semantic.warning.text, ...typography.bodySmall }}>Import blocked by source eligibility. Resolve every invalid record, runtime artifact, secret, and control-plane issue first.</p> : null}
      </Panel>
      {operation ? <Panel title="Operation state" description={operation.error?.detail ?? operation.error?.title ?? 'Server-authoritative operation state.'}><Status label={operation.status} tone={operation.status === 'completed' ? 'success' : operation.status === 'failed' || operation.status === 'cancelled' ? 'danger' : 'info'} live /></Panel> : null}
      {error ? <Panel title="Migration failed" description={error} actions={<Button onClick={() => setError(undefined)}>Dismiss</Button>} /> : null}
    </div>
  </WebShell>;
}

function InspectSummary({ report, snapshot }: { report: MigrationInspectReport; snapshot: MigrationSourceSnapshot }) {
  return <Panel title="Inspection result" description={`Source hash ${report.sourceTreeSha256}`} actions={<Status label={report.cutoverEligibility} tone={report.cutoverEligibility === 'eligible' ? 'success' : 'danger'} />}><div style={{ display: 'grid', gap: spacing[3] }}><div style={{ display: 'grid', gap: spacing[2], gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))' }}>{Object.entries(report.entityCounts).map(([name, count]) => <div key={name}><div style={typography.caption}>{name}</div><strong>{count.toLocaleString()}</strong></div>)}<div><div style={typography.caption}>Files</div><strong>{snapshot.files.length.toLocaleString()}</strong></div><div><div style={typography.caption}>Bytes</div><strong>{snapshot.totalBytes.toLocaleString()}</strong></div></div><p style={{ margin: 0, ...typography.bodySmall }}>Runtime findings: {report.runtimeFindings.map(finding => `${finding.runtime} (${finding.class}, ${finding.records})`).join(', ') || 'none'}.</p><p style={{ margin: 0, ...typography.bodySmall }}>Invalid records: {report.invalidRecords.length}; warnings: {report.warnings.length}; dangling references: {report.danglingReferences.length}; secret fields: {report.secretScan.secretFieldsPresent}.</p></div></Panel>;
}

async function waitForOperation(api: WebApiClient, operationId: string, update: (operation: Operation) => void): Promise<Operation> {
  for (let attempt = 0; attempt < 300; attempt += 1) {
    const operation = await api.reads.operation(operationId, new AbortController().signal);
    update(operation);
    if (operation.status === 'completed') return operation;
    if (operation.status === 'failed' || operation.status === 'cancelled') throw operation.error ?? new Error(`Migration operation ${operation.status}.`);
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  throw new Error('Migration operation timed out. Check operation state before retrying.');
}

function parseInspectResult(value: unknown): MigrationInspectResult | undefined {
  if (Array.isArray(value) && value.length === 2 && isRecord(value[0]) && isRecord(value[1])) return { report: value[0] as MigrationInspectReport, snapshot: value[1] as MigrationSourceSnapshot };
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const object = value as Record<string, unknown>;
  if (isRecord(object.report) && isRecord(object.snapshot)) return { report: object.report as MigrationInspectReport, snapshot: object.snapshot as MigrationSourceSnapshot };
  return undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> { return Boolean(value && typeof value === 'object' && !Array.isArray(value)); }

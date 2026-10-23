# One-Pass Migration, Cutover, and Rollback Runbook

## Command contract

Names define required semantics; implementation may wrap them.

```text
buglogin-migrate export-control-plane --expected-database <source-db-name> --source-instance-id <stable-source-id> --storage-policy <workspace-storage-policy.json> --key-namespace <namespace> --key-name <name> --output <new-control-plane-export.json> --read-only
buglogin-migrate prepare-source --source <root> --control-plane <export.json> --output <staging-source> --read-only
buglogin-migrate hydrate-s3-profile-payloads --source <staging-source> --read-only
buglogin-migrate inspect --source <root> --report <new-dir>
buglogin-migrate snapshot --source <root> --output <new-file> --read-only
buglogin-migrate export --report <inspect.json> --snapshot <snapshot.json> --output <new-dir> --created-at <UTC> --tool-commit <sha> --redacted --signing-key-namespace <namespace> --signing-key-name <name> --signing-key-id <id> --signer <actor> --signed-at <UTC>
buglogin-migrate export --source <root> --report <inspect.json> --snapshot <snapshot.json> --output <new-dir> --created-at <UTC> --tool-commit <sha> --key-namespace <namespace> --key-name <name> --signing-key-namespace <namespace> --signing-key-name <name> --signing-key-id <id> --signer <actor> --signed-at <UTC>
buglogin-migrate verify --package <dir> --signing-key-namespace <namespace> --signing-key-name <name>
buglogin-migrate import --package <dir> --destination <root> --staging <root> --run-id <id> --key-namespace <namespace> --key-name <name> --signing-key-namespace <namespace> --signing-key-name <name> --confirm CUTOVER_AND_PROMOTE
buglogin-migrate import --package <dir> --destination <root> --staging <root> --run-id <id> --key-namespace <namespace> --key-name <name> --signing-key-namespace <namespace> --signing-key-name <name> --discard-staging
buglogin-migrate reconcile --source-report <inspect.json> --destination <root> --run-id <id>
buglogin-migrate rollback --destination <root> --cutover-id <id> --confirm ROLLBACK_MIGRATION_RUN
```

`export-control-plane` reads `BUGLOGIN_LEGACY_DATABASE_URL` from the process environment. The URL must point to loopback through an authenticated tunnel and its database name must exactly match `--expected-database`. The command uses one repeatable-read, read-only transaction with bounded statement and lock timeouts. Legacy password salt/hash fields are encrypted in memory with the migration key before they enter the control-plane export; plaintext credential fields are never written to disk, reports, or CLI output. OAuth subjects, invite tokens, and billing secrets are never selected.

The storage policy is a separate reviewed JSON file with exact coverage for the cloned workspaces. No storage quota is inferred from incomplete S3 path matches:

```json
{
  "schema": "buglogin.legacy-storage-policy/1",
  "workspace_max_storage_bytes": {
    "<workspace-id>": 1073741824
  }
}
```

Replace the example key with every workspace ID from the clone and set each quota from reconciled object inventory plus the approved capacity policy. Missing, zero, or extra workspace entries fail closed. Active shares for a direct workspace member remain grants. Pending invitations are written to `reports/pending-invite-provenance.jsonl` with hashed identity, role, expiry, and creator data; their bearer tokens are omitted and new 3.0 invitations must be issued. An active share aimed at exactly one unconsumed invite becomes a checksummed `reports/pending-share-provenance.jsonl` entry cross-checked against that invite; import does not activate it. Ambiguous invite matches block export.

`prepare-source` copies the local profile snapshot without changing it, rejects symlinks and output paths inside the source, validates and deterministically canonicalizes the separately exported control plane, and writes `control-plane-export.json` only into the prepared copy. Next, `hydrate-s3-profile-payloads` reads the 1.0 manifests and files from S3 using `BUGLOGIN_LEGACY_S3_ENDPOINT`, `BUGLOGIN_LEGACY_S3_BUCKET`, `BUGLOGIN_LEGACY_S3_ACCESS_KEY`, `BUGLOGIN_LEGACY_S3_SECRET_KEY`, and the optional `BUGLOGIN_LEGACY_S3_REGION`, `BUGLOGIN_LEGACY_S3_ROOT_PREFIX`, `BUGLOGIN_LEGACY_S3_PATH_STYLE`, and `BUGLOGIN_LEGACY_S3_SESSION_TOKEN`. It performs only S3 GETs, verifies manifest/profile/workspace identity, relative paths, per-file size and BLAKE3, and hydrates valid cloud payloads into the prepared copy. Invalid or missing payloads use a non-empty local copy when available; otherwise the profile remains preserve-only. Temporary downloaded files live under the OS temp directory and are removed after each profile. Run `inspect`, `snapshot`, and `export` against the hydrated prepared copy; never against a live production root.

`export` defaults to encrypted profile data and reads exactly 32 encryption-key bytes from the native OS vault. Every production package is signed with a separate vault-backed HMAC-SHA256 key and `signature.json` authenticates the checksum-manifest hash. `--redacted` produces the neutral report-only package. `--unsigned` and `verify --checksums-only` are sanitized-fixture options only and are forbidden as release evidence. Plaintext key arguments and environment variables are unsupported. Proxy credentials and VPN configs receive deterministic vault references; secret values are authenticated encrypted objects and never occur in entity JSON, reports, filenames, errors, or debug output. `import --dry-run` decrypts and validates the package, checks destination vault/entity/artifact create/reuse/collision state, builds the complete profile-resource-grant graph, then reports all IDs without creating destination records. Import stages workspaces/memberships, vault objects and artifacts, groups/proxies/extensions/extension groups/VPNs, profiles, resources, `profileResource` bindings, then grants. Activation remains unavailable until graph validation passes; metadata activation is atomic, with grants last. Failure cleanup and confirmed rollback remove/quarantine only journal-owned data in reverse dependency order using vault/entity/artifact CAS: grants, profile-resource bindings, resources, profiles, dependent entities, artifacts, vault objects, memberships/workspaces. `reconcile` proves vault version/hash, artifact checksum/path, entity counts/revisions/SHA-256, profile-resource edge uniqueness/workspace/archive consistency, profile references, and grant-to-binding coverage. `sync-export.json` manifest/blob/tombstone/revision envelopes are preserved and checksum-verified inside migration packages. Hosted sync import already promotes manifest state, migration journal, audit entry, and idempotency receipt through one PostgreSQL control transaction after staged blobs verify; the route also probes the durable idempotency receipt after an uncertain response. Production acceptance remains blocked on live PostgreSQL atomicity/unknown-outcome proof, safe orphan-blob retention and GC, restart recovery evidence, and the remaining sync-import release blockers in `production-launch-sync-v1.yaml`. PostgreSQL is the only hosted manifest visibility authority; Redis remains lease/fencing/event infrastructure and must never receive a second authoritative migration manifest commit. This runbook is not production cutover authorization.

Commands return non-zero on infrastructure or package-integrity failures and write JSON reports. S3 per-profile payload failures are counted and leave the profile on a verified local fallback or preserve-only. No command modifies the original local snapshot or writes/deletes S3 objects; hydration writes only to the prepared copy.

Control-plane packages containing `entities/accounts.jsonl` must use the authenticated `POST /v1/migrations/import` flow. The standalone CLI rejects those packages to prevent a false-success local import that omits workspace, member, resource, binding, or grant state. The API flow requires `destinationRoot` to equal the active profile repository root and applies local plus control state with compensating rollback on control activation failure.

Control import and package rollback acquire the same PostgreSQL session advisory lock derived from the package ID before entering the local control-mutation gate. Concurrent requests for one package fail with `409 migration_control_import_in_progress`; requests for other packages remain independently addressable. Durable idempotency receipts still govern completed retries, and the database snapshot/rollback checkpoint remains the recovery boundary for the single direct clone. No repeated rehearsal cycle is required.

## Roles

| Role | Responsibility | Cannot approve alone |
|---|---|---|
| Migration lead | Timeline, commands, evidence | Reconciliation exception |
| Source owner | Freeze source, validate snapshot | Destination acceptance |
| Destination owner | Capacity/version acceptance | Source completeness |
| Security reviewer | Secrets, signing, keys | Functional acceptance |
| QA lead | E2E cohorts/evidence | Data exception |
| Release commander | Go/no-go/rollback | Unreviewed stop override |

Cutover needs release commander plus source/destination owner. ACL exceptions also need security approval.

## One-pass migration prerequisites

- Supported source version.
- Read-only, consistent source export; the 1.0 service may remain online during the clone.
- Destination build/schema pinned.
- Package schema/signing key pinned.
- Run the consolidated verification once after implementation is frozen, as requested.
- Free space: at least `2.5 × source logical bytes + installer/runtime reserve`.
- Destination pre-state recovery copy.
- Secret canaries.
- Cohorts: critical, large, proxy-auth, cookie-heavy, extension-heavy, ACL, unsupported-runtime.

## One-pass diff and clone

1. Record source version/build, counts, bytes, and source identity.
2. Take one read-only source-to-target diff; list every missing, extra, transformed, and unsupported record.
3. Create one immutable, consistent DB/profile snapshot and record the destination pre-state. Do not require a source write-freeze for this low-volume direct clone; changes made in 1.0 after the snapshot are outside this clone.
4. Export/map once; verify package paths, checksums, signature, and redaction.
5. Review deterministic ID mapping, collisions, unsupported records, profile-resource graph, ACL, and capacity against the diff.
6. Import once into the isolated 3.0 destination.
7. Reconcile source and destination counts, references, object hashes, effective ACL, and every profile-resource edge.
8. Keep the source snapshot and destination pre-state checkpoint for rollback; do not repeat rehearsals or imports.
9. Archive the diff, signed package evidence, reconciliation result, and unresolved exclusions.

## Acceptance

- Source before/after hash unchanged.
- Every profile gets deterministic result.
- Every accepted legacy cloud `prf_<32-hex>` maps to the same lowercase canonical UUID in profile and resource namespaces, with one explicit binding; malformed or duplicate mappings are zero.
- Required counts exact; exclusions match ledger.
- Managed/shared profile count equals active plus archived profile-resource binding count; duplicate, dangling, cross-workspace, archive-state mismatch, and unbound profile-grant counts are zero.
- Migrated object hashes: 100% match.
- Effective ACL differences: zero unless individually approved.
- Critical cohorts launch/stop/relaunch; cookies/extensions/proxy persist.
- Unsupported runtimes remain recoverable and blocked.
- Secret canaries absent from logs/reports/temp/CI/screenshots.
- Rollback restores the recorded destination pre-state if reconciliation fails.

The one-pass evidence must record the source snapshot hash, destination pre-state hash, package ID, expected/activated counts, reconciliation result, run ID, and redaction status. Evidence must not contain source paths, profile bytes, credentials, or secret canaries. The migration command must reject incorrect cutover/rollback confirmations before writes.

## Cutover preparation

- Pin destination schema/migration code for the one-pass import.
- Publish signed RC/checksums.
- Do not require an old-client write block for the data clone; document the source snapshot time so any later 1.0 changes can be handled explicitly.
- Recalculate capacity from latest inspect.
- Validate backups/restore timing.
- Assign cutover ID/evidence directory.
- Set the rollback deadline.
- Freeze transformation rules before the one-pass export; a change after the source snapshot invalidates that package and requires a new diff/snapshot before writing.

## Cutover

1. Announce maintenance; block new sessions/jobs.
2. Drain browser, sync, API, MCP, background jobs.
3. Capture the agreed source snapshot while the low-volume 1.0 service remains online; record its timestamp and counts.
4. Record the snapshot's tree/object hashes and destination pre-state.
5. Export and sign that snapshot once.
6. Export, sign, verify, and review the diff.
7. Import once to destination staging paths.
8. Reconcile counts, hashes, references, ACL, unsupported ledger, exact `prf` mapping, profile-resource graph, archive states, and grant coverage.
9. Promote metadata/objects atomically only if reconciliation is exact and the diff has no unapproved exclusions.
10. Verify the rollback checkpoint is available.
11. Enable reads and observe diagnostics/support visibility.
12. Enable 3.0 writes; retain the original 1.0 source and snapshot unchanged for rollback. Writes made in 1.0 after the snapshot are not included in 3.0.
13. Close only after reconciliation and rollback evidence are archived.

## Stop conditions

Stop before write enablement for source mutation/hash drift, unknown version, missing deterministic result, malformed or colliding `prf` mapping, duplicate/dangling/cross-workspace profile-resource edge, archive-state mismatch, unbound profile grant, secret leakage, hash mismatch, unapproved ACL difference, old-client writes, insufficient capacity, critical launch/cookie/extension/proxy/sync failure, unavailable rollback, or destination signing/notarization failure.

## Rollback decision

Rollback is mandatory for data loss, permission expansion, unrecoverable critical launch failure, systematic profile/cookie corruption, or destination write instability. Forward-fix requires immutable source, recoverable destination, no security/data-loss condition, and owner approvals.

## Rollback

1. Disable destination writes; drain jobs.
2. Preserve destination divergence as signed encrypted incident export; never auto-merge to source.
3. Remove or quarantine journal-owned grants, profile-resource bindings, resources, profiles, dependent entities, artifacts, vault objects, memberships, and workspaces in that reverse dependency order using CAS; preserve all pre-existing rows and objects.
4. Restore destination pre-cutover pointers/state using cutover journal.
5. Verify destination pre-state hash plus zero surviving journal-owned profile-resource edges or grants.
6. Re-enable old source known-good path.
7. Remove freeze only after rejected/accepted write probes pass.
8. Verify legacy launch, proxy, cookies, sync, API, ACL.
9. Notify users with exact affected window/divergence process.
10. Retain snapshots, packages, maps, checksums, graph reports, logs.
11. Close the migration after reconciliation and rollback evidence are archived; no repeat rehearsal cycle is required.

## Evidence package

Approvals, versions, source/destination hashes, package signature, command reports, ID map, exact `prf` mapping report, counts, exclusion ledger, profile-resource graph report, archive-state reconciliation, grant coverage, ACL comparison, E2E results, screenshots, rollback checkpoint, monitoring summary, residual-risk sign-off.

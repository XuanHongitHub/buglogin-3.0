# Migration Rehearsal, Cutover, and Rollback Runbook

## Command contract

Names define required semantics; implementation may wrap them.

```text
buglogin-migrate prepare-source --source <root> --control-plane <export.json> --output <staging-source> --read-only
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

`prepare-source` is the required first step for a real legacy export. It copies the source without mutation, rejects symlinks and output paths inside the source, validates and deterministically canonicalizes the separately exported control plane, writes `control-plane-export.json` only into the prepared copy, and proves the source tree hash is unchanged. Run `inspect`, `export`, and all rehearsals against the prepared copy; never against a live production root.

`export` defaults to encrypted profile data and reads exactly 32 encryption-key bytes from the native OS vault. Every production package is signed with a separate vault-backed HMAC-SHA256 key and `signature.json` authenticates the checksum-manifest hash. `--redacted` produces the neutral report-only package. `--unsigned` and `verify --checksums-only` are sanitized-fixture options only and are forbidden as release evidence. Plaintext key arguments and environment variables are unsupported. Proxy credentials and VPN configs receive deterministic vault references; secret values are authenticated encrypted objects and never occur in entity JSON, reports, filenames, errors, or debug output. `import --dry-run` decrypts and validates the package, checks destination vault/entity/artifact create/reuse/collision state, builds the complete profile-resource-grant graph, then reports all IDs without creating destination records. Import stages workspaces/memberships, vault objects and artifacts, groups/proxies/extensions/extension groups/VPNs, profiles, resources, `profileResource` bindings, then grants. Activation remains unavailable until graph validation passes; metadata activation is atomic, with grants last. Failure cleanup and confirmed rollback remove/quarantine only journal-owned data in reverse dependency order using vault/entity/artifact CAS: grants, profile-resource bindings, resources, profiles, dependent entities, artifacts, vault objects, memberships/workspaces. `reconcile` proves vault version/hash, artifact checksum/path, entity counts/revisions/SHA-256, profile-resource edge uniqueness/workspace/archive consistency, profile references, and grant-to-binding coverage. `sync-export.json` manifest/blob/tombstone/revision envelopes are preserved and checksum-verified inside migration packages. Hosted sync import remains blocked until PostgreSQL-backed staging journal, single-transaction manifest promotion/idempotency/audit, ambiguous-commit probing, memory rehydration, and orphan-blob GC are implemented and tested. PostgreSQL is the only hosted manifest visibility authority; Redis remains lease/fencing/event infrastructure and must never receive a second authoritative migration manifest commit. This runbook is not production cutover authorization.

Every command returns non-zero on incomplete verification and writes JSON reports. No command modifies source.

Control-plane packages containing `entities/accounts.jsonl` must use the authenticated `POST /v1/migrations/import` flow. The standalone CLI rejects those packages to prevent a false-success local import that omits workspace, member, resource, binding, or grant state. The API flow requires `destinationRoot` to equal the active profile repository root and applies local plus control state with compensating rollback on control activation failure.

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

## Rehearsal prerequisites

- Supported source version.
- Immutable access-controlled clone; no production access from development tools.
- Destination build/schema pinned.
- Package schema/signing key pinned.
- Fixture suite passing Windows/macOS.
- Free space: at least `2.5 × source logical bytes + installer/runtime reserve`.
- Destination pre-state recovery copy.
- Secret canaries.
- Cohorts: critical, large, proxy-auth, cookie-heavy, extension-heavy, ACL, unsupported-runtime.

## Rehearsal

1. Record source version/build, OS, root, counts, bytes, tree hash.
2. Run `inspect`; resolve unknown versions, invalid records, secret findings.
3. Create/hash immutable snapshot.
4. Export package; verify paths, checksums, signature, redaction.
5. Dry-run; review ID map, exact legacy `prf` conversions, collisions, unsupported records, profile-resource graph, ACL, capacity.
6. Import into isolated staging root.
7. Reconcile counts, references, object hashes, effective ACL, and every profile-resource edge.
8. Run migration cohorts on both OS families.
9. Re-import; require zero creates, duplicate objects, unintended updates.
10. Roll back; require exact destination pre-state.
11. Import again; require identical IDs/results.
12. Archive signed evidence and risks.

Two complete rehearsals required. Rehearsal 2 uses release-candidate binary and final runbook.

### Synthetic control-plane rehearsal evidence

The synthetic rehearsal must include an ACL-complete export equivalent to `MIG-018-acl-complete` and prove, in one test run:

- canonical control-plane key ordering and rejection of forbidden secret/token fields;
- exact accounts, organizations, workspaces, teams, members, resources, `profileResource` bindings, and grant counts;
- signed-package verification before any import planning;
- local dry-run creates no profile, entity, vault, or control-plane state;
- source snapshot remains byte-equivalent before and after the rehearsal.

This proves the package/export boundary only. It does not prove hosted control-plane import, PostgreSQL transactionality, API authorization, or production cutover readiness.

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
- Rollback and repeat import pass.

Synthetic evidence must also record the source tree hash, destination pre-state hash, package ID, expected/activated counts, reconciliation result, run ID, and redaction status. Evidence must not contain source paths, profile bytes, credentials, or secret canaries. The rehearsal must reject incorrect cutover/rollback confirmations before writes and prove repeated activation rollback is safe.

## Cutover preparation

- Freeze destination schema/migration code.
- Publish signed RC/checksums.
- Confirm old-client write block.
- Confirm contacts, maintenance notice, decision channel, evidence location.
- Recalculate capacity from latest inspect.
- Validate backups/restore timing.
- Assign cutover ID/evidence directory.
- Set maximum freeze and rollback deadline.
- Any transformation-rule change forces another rehearsal.

## Cutover

1. Announce maintenance; block new sessions/jobs.
2. Drain browser, sync, API, MCP, background jobs.
3. Freeze source writes server/client-side; verify rejected writes.
4. Record final counts/tree/object hashes.
5. Create final immutable snapshot.
6. Export, sign, verify, dry-run.
7. Evaluate stops; obtain go approval.
8. Import to destination staging paths.
9. Reconcile counts, hashes, references, ACL, unsupported ledger, exact `prf` mapping, profile-resource graph, archive states, and grant coverage.
10. Promote metadata/objects atomically.
11. Run critical Windows/macOS smoke and migration cohorts.
12. Enable reads only; validate diagnostics/support visibility.
13. Verify rollback checkpoint.
14. Enable writes.
15. Keep source immutable; monitor through rollback deadline.
16. Close only after evidence sign-off.

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
11. Require new rehearsal cycle.

## Evidence package

Approvals, versions, source/destination hashes, package signature, command reports, ID map, exact `prf` mapping report, counts, exclusion ledger, profile-resource graph report, archive-state reconciliation, grant coverage, ACL comparison, E2E results, screenshots, rollback checkpoint, monitoring summary, residual-risk sign-off.

# Migration Production-Readiness Evidence Register

Status: audit snapshot, 2026-10-13

This register separates evidence that can be produced from sanitized fixtures from evidence that requires a production clone, approved staging services, or a real cutover window. No production data, source root, credentials, or production claim is created by this document.

## Fastest offline artifacts

| Artifact | Current evidence | Fastest action | Release meaning |
|---|---|---|---|
| Synthetic package-to-rollback report | Present in `crates/migration/tests/rehearsal.rs` | Keep the evidence schema assertions green | Synthetic boundary only |
| ACL-complete synthetic rehearsal | Present for `MIG-018-acl-complete` | Re-run with the release-candidate commit and archive JSON output | Control-plane mapping confidence |
| Negative confirmation evidence | Present for cutover and rollback confirmation | Retain error codes in the rehearsal artifact | Prevents accidental writes |
| Repeat activation and rollback evidence | Present in synthetic rehearsal and rollback retry test | Add both outputs to the RC evidence bundle | Idempotency/rollback confidence |
| Fixture coverage index | Present: 28 fixtures | Run the full migration fixture suite and archive the command log | Sanitized regression breadth |
| Production-readiness manifest | Missing before this register | Generate one per RC with explicit `proven`/`blocked` fields | Prevents evidence overclaiming |

## Isolated live-source rehearsal snapshot — 2026-10-13

This is a database-restore rehearsal from the legacy 1.0 control plane, performed while the source API remained online. It proves that this point-in-time database dump can be restored into an isolated database on the BugIdea host; it does **not** prove write-freeze, complete profile-object migration, application compatibility, or cutover readiness.

| Check | Evidence | Result |
|---|---|---|
| Legacy source | PostgreSQL 16.15 database `buglogin`; 26 public tables; database size 10,820,631 bytes | Read-only source; 1.0 API remained online |
| Snapshot artifact | Custom-format `pg_dump`, `--no-owner --no-privileges`; 166,661 bytes; SHA-256 `e1341386a5eaf2ce5ff4aa68fe5335e18f0a248d008d70de9331a8f67a080de6`; `pg_restore --list` succeeded | Verified on workstation and both VPS hosts; owner-restricted copies |
| Destination | `buglogin3_rehearsal_20261013` on the isolated BugIdea PostgreSQL instance; restored in one transaction with `--exit-on-error --single-transaction --no-owner --no-privileges` | 26 public tables restored |
| Row-count reconciliation | Exact per-table count comparison between source snapshot and rehearsal database | All 26 tables matched; zero count differences |
| Access isolation | Database ACL grants `CONNECT` only to owner `postgres`; `PUBLIC` and role `buglogin3` cannot connect | Passed; BugLogin 3 API remained inactive |
| Source/service integrity | Source database was not modified; old `buglogin-api` PM2 service remained online | Passed for this rehearsal window |

The source was live during `pg_dump`; PostgreSQL's dump is a consistent snapshot, but there was no application write-freeze and therefore no claim that the dump is the final cutover state. The rehearsal database is intentionally inaccessible to the BugLogin 3 service identity. This artifact covers control-plane PostgreSQL only; S3/profile payloads, key distribution, migration transforms, API-level reads, rollback against a populated destination, and native desktop release evidence remain unproven.

## Legacy control-plane and S3 inventory — 2026-10-13

Read-only queries against the same live 1.0 source found 49 workspaces (37 personal, 12 team), 34 hosted profile rows, 91 memberships, 42 share grants, and 14 workspace groups. The `workspaces` schema has no organization relationship or organization table. Profile runtime metadata is mixed: 21 `bugmini`, 4 `bugox`, and 9 `cloak`. The local 1.1.84 data-root snapshot contains only 14 valid profile metadata rows and is not a complete source for this hosted control plane.

Read-only S3 `ListObjectsV2` inventory used the existing 1.0 server credential without emitting credential values, object keys, or object contents:

| Prefix | Objects | Bytes | Migration interpretation |
|---|---:|---:|---|
| `buglogin-prod` | 6,801 | 858,029,757 | Includes `workspaces`: 6,729 objects / 858,000,006 bytes; candidate production sync data, linkage not complete |
| `profiles` | 10,578 | 1,786,636,841 | Separate root; key layout not yet reconciled to hosted profile IDs |
| `workspaces` | 8,351 | 1,495,646,206 | Separate root; key layout not yet reconciled to hosted workspace IDs |
| `releases` | 164 | 11,242,861,871 | Browser/desktop release artifacts; must not be included in profile migration |
| `selfhost-dev` | 17,679 | 1,848,800,956 | Development/self-host data; must not be included in production migration |
| Other (`meta`, `proxies`) | 25 | 12,241 | Requires explicit ownership/purpose mapping |
| **Total** | **43,598** | **17,231,987,872** | Inventory only; payload snapshot/import not performed |

Within the candidate `buglogin-prod/workspaces` path, only 23 of the 34 database profile IDs and 13 of the 49 workspace IDs appear as exact path segments. These counts do not prove that missing records have no data elsewhere or that matching objects are complete. Relationship validation found one active share grant whose recipient is an unconsumed invite rather than a direct workspace member; the current migration mapper correctly fails closed for that grant. Organization synthesis and pending-invite/grant treatment require explicit deterministic migration rules before a complete export can be produced.

This S3 credential is shared with 1.0 and can list multiple production, development, profile, workspace, and release prefixes. A `buglogin3/prod` write prefix is not an isolation boundary. No S3 payload was downloaded, changed, or deleted during this inventory.

## Still blocked by environment or production controls

| Gate | Why offline fixtures cannot prove it | Required artifact |
|---|---|---|
| Production-clone export | A stable local BugLogin 1.1.84 profile-root clone was inspected, but it has only 14 profile metadata rows versus 34 hosted profile rows. The live control schema has no organizations table, one active grant targets an unconsumed invite, and only 23/34 profile IDs plus 13/49 workspace IDs appear in the candidate S3 prefix | Approved deterministic organization and invite/grant mapping rules; immutable combined source snapshot; complete S3/profile/control-plane reconciliation; signed package and before/after source hash |
| Source write freeze | Requires the old client/server write path and an approved freeze window | Rejected-write probes, timestamps, operator sign-off |
| Two complete RC rehearsals | Requires release-candidate binaries and final runbook execution | RC-1 and RC-2 evidence bundles with distinct run IDs |
| Hosted PostgreSQL/Redis/S3 migration path | Requires trusted staging endpoints and real adapter coordination | Transaction, lease, recovery, and orphan-GC evidence |
| Production rollback checkpoint | Requires destination pre-state and a cutover journal from a real clone | Signed checkpoint, rollback result, post-rollback reconciliation |
| Windows/macOS migration smoke | Requires native signed/notarized artifacts | Platform-specific launch, profile, proxy, cookie, extension, sync results |

These blocked gates remain blocked. Synthetic tests must not be relabeled as production evidence.

## RC evidence bundle

Each sanitized RC run should archive only:

1. `manifest.json`: schema, evidence class, run ID, commit, tool version, fixture IDs.
2. `source.json`: source identity and tree hash; no absolute source path.
3. `package.json`: package ID, checksum-manifest hash, signature verification result.
4. `activation.json`: expected/created/reused counts and idempotency result.
5. `reconciliation.json`: counts, hashes, graph checks, ACL result, exclusion result.
6. `rollback.json`: activation rollback, cutover rollback, repeat behavior, retained foreign artifacts.
7. `redaction.json`: secret scan result and forbidden-string result.
8. `commands.txt`: exact commands, exit codes, and test summary.

Required manifest values for sanitized runs:

```json
{
  "schema": "buglogin.migration-evidence/1",
  "evidenceClass": "synthetic-only",
  "productionReadiness": "not-proven",
  "sourceAccess": "sanitized-fixture",
  "secrets": "absent",
  "sourceMutation": "not-observed",
  "hostedTransactionality": "not-tested",
  "rollback": "passed"
}
```

`productionReadiness: ready` is forbidden for this evidence class. A production bundle may use that value only after every blocked gate above has independently passed and received operator approval.

## Suggested offline commands

```text
cargo test -p buglogin-migration --test rehearsal
cargo test -p buglogin-migration --test rollback_retry
cargo test -p buglogin-migration --test fixtures
cargo test -p buglogin-migration --test cli
```

The commands produce test evidence only. They do not access `E:\bug-login`, any production database, hosted object storage, or live user data.


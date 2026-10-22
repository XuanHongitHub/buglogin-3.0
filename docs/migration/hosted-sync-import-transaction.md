# Hosted Sync Import Transaction

- Status: implementation contract; release blocked until every acceptance case passes.
- Authority: PostgreSQL durable control state only.
- Supporting systems: Redis lease/fencing/events; object storage immutable encrypted blobs.

## Non-negotiable invariants

1. Production desktop never starts or falls back to a local server/repository. All authoritative sync mutations use the hosted HTTPS API.
2. Local/self-hosted mode remains standalone and never writes production control state.
3. PostgreSQL is the only authority deciding which manifest is current. Redis manifest payloads are never read or written by hosted migration activation.
4. Source fencing tokens are data only. Destination Redis issues a new fencing token.
5. Blob upload never makes data visible. Visibility begins only when PostgreSQL commits the target manifest.
6. The destination base generation and canonical checksum are checked again inside the final PostgreSQL transaction.
7. A failed or ambiguous final commit never triggers immediate blob deletion. Cleanup requires proven non-activation plus retention.
8. Retry with the same idempotency key and export hash returns the durable receipt. Reuse with different input fails.

## Durable journal

Record type: `migration_sync_import`. Record ID: canonical SHA-256 of actor, workspace, resource, and idempotency key.

Required fields:

- `schema`: `buglogin.migration-sync-journal/1`
- `operationId`, `exportId`, `exportSha256`, `manifestSha256`, `blobSetSha256`
- `workspaceId`, `resourceId`, `baseGeneration`, `targetGeneration`
- `state`: `staging | finalizing | active | cleanup_pending | abandoned`
- `createdObjects[]`: storage key, checksum, size
- `resumedObjects[]`: storage key, checksum, size
- `reservedObjects[]`: storage key, checksum, size for objects whose workspace quota is reserved by this attempt; absent in older journals
- `destinationFencingToken`; never persist owner token or bearer token
- `controlRevisionBefore`, `controlRevisionAfter`
- `createdAt`, `updatedAt`, terminal/error code

State transitions:

```text
new -> staging -> finalizing -> active
                    |
                    +-> cleanup_pending -> abandoned
```

No transition may move `active` backward. `cleanup_pending` means activation is proven absent; it is not a rollback of committed production state.

## Import algorithm

1. Authenticate the request and validate the complete `SyncExport`: schema, revision chain, scope, manifest checksum, exact blob set, sizes, checksums, tombstones.
2. Refresh the PostgreSQL-backed authorization snapshot and reauthenticate; verify `migration.write`, workspace/resource/profile-resource binding, effective ACL, and proxy dependencies.
3. Acquire a PostgreSQL advisory lock derived from the canonical idempotency key, then claim durable idempotency using the full request hash. A concurrent copy of the same request receives `409 migration_sync_import_in_progress`; a later retry can resume the durable journal after the first request releases the lock.
4. Insert/replay the `staging` journal, persist this attempt's `reservedObjects[]`, and atomically reserve their combined workspace quota before object-store writes. Older journals deserialize with an empty reservation list.
5. Acquire destination Redis lease idempotently; retain owner token only in Rust memory.
6. Refresh the control aggregate again while holding the mutation lock; reauthenticate the actor and recheck migration ACL, active binding, and proxy dependencies before any object-store write.
7. Read the current destination manifest from PostgreSQL-backed control state and plan genesis/successor/replay; remap the destination fencing token. Because the export contains and verifies every manifest object's bytes, clear each source `sourceGeneration` when building the destination manifest: all objects are staged at the destination manifest generation. Source generation describes source-store placement only.
8. Upload content-addressed blobs. Existing equal-checksum objects are resumed; collisions fail. Append created/resumed evidence durably.
9. Verify every object through the remote adapter after upload.
10. Mark journal `finalizing`.
11. Execute one PostgreSQL transaction that:
   - locks/checks expected control revision;
   - rechecks current workspace/resource generation and canonical checksum;
   - writes the immutable target control aggregate containing the target manifest;
   - marks the journal `active`;
   - removes quota reservations for objects promoted into that active manifest;
   - writes audit;
   - completes the durable idempotency receipt.
12. Treat the PostgreSQL commit as the only linearization point.
13. Rehydrate/swap in-memory control state from the committed aggregate. Failure drains the node; it never writes the old projection back.
14. Release Redis lease and publish projection events best-effort. Their failure does not reverse an active import.

## Ambiguous commit recovery

If PostgreSQL commit returns a connection/unknown-outcome error:

1. Do not delete uploaded blobs.
2. Open a fresh persistence connection.
3. Probe durable idempotency receipt and journal.
4. `active` plus matching request hash returns the stored success receipt.
5. `finalizing` without committed target manifest remains recovery-required; background reconciliation repeats the probe and never activates a second manifest.
6. Proven absent transaction may mark `cleanup_pending`; GC handles unreferenced objects after retention.

## Object cleanup

- No synchronous delete after an ambiguous commit.
- GC selects only journal-created objects not referenced by any active/retained manifest.
- GC rechecks checksum and reference set immediately before CAS delete.
- Resumed/pre-existing objects are never journal-owned and never deleted by migration cleanup.
- A cleanup transition releases the attempt's quota reservations after the journal state is durable; active manifests and other staging attempts keep their accounting.
- Retention and two-pass confirmation are mandatory for production.

## Required acceptance tests

- Fresh genesis; contiguous successor; idempotent replay.
- Import reused objects with `sourceGeneration`; verify destination manifest references the destination generation and replay treats source placement as non-semantic.
- Same key/different request conflict.
- Same key/same request concurrent across API instances has one active journal writer; retry after the first request exits remains resumable.
- Wrong base, stale generation, scope/binding/ACL denial.
- Missing, extra, duplicate, tampered blob rejection before activation.
- Source fencing token not reused.
- Mid-upload failure leaves old manifest current.
- Two API instances compete on one base; one PostgreSQL winner.
- Normal sync writer competes with migration; no lost generation.
- Every final SQL statement failure rolls back aggregate, journal, audit, receipt.
- Commit succeeds but adapter returns unknown; fresh probe returns success; no blob delete.
- Crash after commit/before memory swap; restart loads target manifest.
- Dropped HTTP response replays durable receipt.
- Redis failure before finalization blocks; Redis failure after PostgreSQL commit does not revert.
- Stale Redis manifest has no effect on hosted reads.
- Created orphan GC; resumed object preservation; checksum-CAS delete.
- Restart recovery for `staging`, `finalizing`, `active`, `cleanup_pending`.

## Release gate

Signed package ingress and the shared REST/MCP package activation path are now implemented in source. Keep `migration_sync_import_not_ready` until the PostgreSQL recovery worker, orphan GC, process-loss/idempotency rehearsal, and all transaction cases above pass against real PostgreSQL, Redis, and S3-compatible adapters. Local tests prove the transaction core only; they do not provide hosted release evidence.

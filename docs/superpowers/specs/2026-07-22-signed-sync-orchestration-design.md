# Signed Package + Hosted Sync Orchestration

- Status: implementation plan
- Date: 2026-07-22

## Invariants

- `production` desktop never invokes local package activation; it uploads migration intent through hosted HTTPS APIs.
- `local` mode may activate filesystem packages and local control state, but never writes hosted authority.
- A package containing `sync/export.json` cannot report success while sync activation is absent.
- Control state, filesystem activation, and hosted sync use explicit rollback/recovery states; ambiguous sync commit or journal-state acknowledgement never deletes staged immutable blobs.
- Replay is keyed by package/run idempotency and sync export hash.

## Sequence

1. Verify signed package and snapshot.
2. Stage/promote/activate local profile files only in local mode.
3. Import control metadata with a durable journal.
4. Import hosted sync through the shared Rust sync service.
5. Return one composite receipt only after both authorities are active.
6. On proven sync failure, rollback control then filesystem; preserve ambiguous blobs for journal recovery.
7. On replay, probe durable receipts before any side effect.
8. Persist each staged object's create intent before PUT; after an ambiguous `Finalizing` journal write, retain the blobs until durable state is reconciled.

## Release gates

- full success and exact replay
- control success + sync failure rollback
- ambiguous sync commit recovery
- wrong-base and idempotency conflict
- production rejects local activation path
- Windows/macOS evidence

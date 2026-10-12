# Signed Package + Hosted Sync Orchestration

Status: implemented locally; production release gates remain open.

1. [x] Extract reusable control rollback core.
2. [x] Extract typed hosted sync import core.
3. [x] Parse and validate package sync export.
4. [x] Orchestrate control, sync, and filesystem rollback.
5. [x] Add success, replay, and failure tests.
6. [x] Checkpoint per-object create intent before immutable PUT and resume exact checksum matches after restart.
7. [x] Preserve staged blobs when the `Finalizing` journal write acknowledgement is ambiguous.
8. [x] Persist lease/preflight failures so non-retryable plan conflicts enter the cleanup lifecycle.
9. [x] Run blocking object-store staging on the blocking pool, renew the destination lease during long transfers, and revalidate write authority before manifest activation.
10. [ ] Run a process-loss rehearsal against trusted PostgreSQL, Redis, and S3 staging adapters.
11. [ ] Prove safe stale-staging recovery and orphan-blob retention/GC behavior.
12. [ ] Run Windows/macOS release evidence.

Local verification for the current checkpoint and ambiguity changes: `cargo test -p buglogin-api`, `node scripts/ci/check-contracts.mjs`, `cargo fmt --all -- --check`, and `git diff --check`. These tests use local/memory adapters and do not close the production gates above.

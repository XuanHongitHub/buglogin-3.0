# Signed Package + Hosted Sync Orchestration

Status: recoverable saga implemented locally; production release gates remain open.

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

## Consistency boundary

- The package activation endpoint commits the control graph before it imports and activates the sync manifest. It compensates by rolling back the control graph on a proven sync failure, and retries use the component idempotency receipts. This is a recoverable saga, not one database transaction spanning both commits.
- Compensation is now limited to the activation attempt that received a newly applied control-import receipt. A concurrent/retry request that sees a replayed receipt cannot roll back control state owned by the earlier attempt if it loses sync lease acquisition. If the original owner loses the lease, it may compensate its own graph; sync import now refreshes and revalidates the active resource binding immediately before manifest publication, so a replay request cannot publish an orphan manifest after that rollback. PostgreSQL control-revision CAS rejects a cross-instance mutation that races after refresh.
- During that interval, control records can be visible before the imported sync manifest is active. A process loss or rollback failure can leave reconciliation work; callers must not describe this as strict atomic visibility.
- `http::package_upload::tests::only_the_attempt_that_created_control_state_can_compensate_it` covers compensation ownership and unknown outcomes. `http_signed_sync_package::signed_sync_package_activation_compensates_and_replays` asserts rollback after a failed first attempt, then races two activation requests with the replay request holding the lease while the original owner loses it. The replay cannot publish a manifest after the owner rolls back its control graph; a subsequent retry reactivates successfully. This is local memory-adapter evidence only. Separate-process visibility, crash recovery, and rollback failure behavior still require trusted PostgreSQL/Redis/S3 rehearsal before production.

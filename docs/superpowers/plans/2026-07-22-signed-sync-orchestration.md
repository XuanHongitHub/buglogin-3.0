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
13. [x] Replace unchecked successor-generation increments with checked arithmetic across API validation, conflict resolution, dry-run migration fencing, and desktop snapshots; regression execution remains deferred to the consolidated pass.
14. [x] Compare Redis manifest generations as canonical decimal strings so Lua floating-point precision cannot collapse distinct `u64` values above 2^53; add a live-adapter regression source, execution deferred to the consolidated pass.
15. [x] Zeroize the temporary bearer copy parsed from sync-import Authorization headers; transfer the owned token directly into `CoordinationAuth`, whose drop handler zeroizes it after the import operation.
16. [x] Replace saturating profile/proxy/control revision and local lease-fence increments with checked errors; add lease sequence regression source, execution deferred to the consolidated pass.
17. [x] Do not seed an empty genesis manifest for the exact profile resource whose signed package includes `sync/export.json`; validate the export identity and let sync import install the authoritative manifest. Keep genesis creation for other imported bindings. Source-current signed-package activation E2E passes retry after lease failure, concurrent activation serialization, replay, and manifest readback.

Local verification for the current checkpoint and ambiguity changes: `cargo test -p buglogin-api`, `node scripts/ci/check-contracts.mjs`, `cargo fmt --all -- --check`, and `git diff --check`. These tests use local/memory adapters and do not close the production gates above.

Local E2E evidence (2026-10-19): three isolated API process scenarios passed using synthetic accounts and temporary SQLite roots: login/credential rejection/profile+proxy CRUD/restart persistence/refresh replay revocation; workspace A↔B isolation; and encrypted cross-device profile/proxy sync with wrong-key/tag rejection, manifest/checksum validation, and restart persistence. The process scenarios use the prebuilt API binary and do not prove later source edits are active in the long-running local API process. Focused source-current API integration suites passed for auth (15), profile (9), proxy (4), groups (3), sync migration (2), and signed package activation (1). The signed activation test proves package retry does not conflict with a generated empty genesis manifest. Desktop bridge/model/workflow self-tests and TypeScript typecheck passed. This does not replace native UI interaction E2E, PostgreSQL/Redis/S3 process-loss rehearsal, or Windows/macOS release evidence.

## Consistency boundary

- The package activation endpoint commits the control graph before it imports and activates the sync manifest. It compensates by rolling back the control graph on a proven sync failure, and retries use the component idempotency receipts. This is a recoverable saga, not one database transaction spanning both commits.
- When a package carries `sync/export.json`, control import leaves genesis creation to every other binding but not the exported resource itself. Sync import installs that resource's authoritative package manifest after acquiring and validating the destination lease. This avoids treating a synthetic generation-1 placeholder as the base for the signed export.
- Compensation is now limited to the activation attempt that received a newly applied control-import receipt. A concurrent/retry request that sees a replayed receipt cannot roll back control state owned by the earlier attempt if it loses sync lease acquisition. If the original owner loses the lease, it may compensate its own graph; sync import now refreshes and revalidates the active resource binding immediately before manifest publication, so a replay request cannot publish an orphan manifest after that rollback. PostgreSQL control-revision CAS rejects a cross-instance mutation that races after refresh.
- During that interval, control records can be visible before the imported sync manifest is active. A process loss or rollback failure can leave reconciliation work; callers must not describe this as strict atomic visibility.
- `http::package_upload::tests::only_the_attempt_that_created_control_state_can_compensate_it` covers compensation ownership and unknown outcomes. `http_signed_sync_package::signed_sync_package_activation_compensates_and_replays` asserts rollback after a failed first attempt, then races two activation requests with the replay request holding the lease while the original owner loses it. The replay cannot publish a manifest after the owner rolls back its control graph; a subsequent retry reactivates successfully. This is local memory-adapter evidence only. Separate-process visibility, crash recovery, and rollback failure behavior still require trusted PostgreSQL/Redis/S3 rehearsal before production.

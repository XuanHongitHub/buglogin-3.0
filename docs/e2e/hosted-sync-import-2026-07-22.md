# Hosted Sync Import Evidence — 2026-07-22

## Scope

Transaction-core evidence for hosted BugLogin 1.x → BugLogin 2.0 sync migration. This evidence does not authorize production cutover.

## Proven behavior

- Production current-manifest authority is PostgreSQL durable control state.
- Redis supplies lease/fencing only; hosted migration never calls Redis manifest commit.
- Object-store blobs stage before visibility.
- Sync export validation rejects missing, extra, duplicate, or tampered blobs and revision chains.
- Destination fencing token replaces the source token.
- Final PostgreSQL transaction writes control aggregate, active migration journal, audit, and idempotent receipt together.
- Durable aggregate/journal/receipt survive store reopen.
- Ambiguous adapter failure after durable commit is recovered by receipt probe and aggregate reload; no rollback/delete occurs.
- Startup recovery scans stale sync journals in two passes. Proven-uncommitted `Finalizing` journals first become `CleanupPending`; only a later stale pass rechecks every active manifest reference, checksum-CAS deletes journal-created objects, then marks `Abandoned`. Referenced, resumed, ambiguous, and delete-failed objects remain preserved; repeated recovery is idempotent.
- `Staging` journals and ambiguous committed state are deferred; no blind blob deletion.
- Remote cleanup failure preserves both the object and `Finalizing` journal for later retry; it never records false recovery.
- Production API startup uses `ProfileService::disabled()`, rejects `BUGLOGIN_DATA_ROOT`, and never opens local profile authority.
- Same idempotency key replays exact result; different request conflicts.
- Wrong destination base rejects without replacing generation 1.
- Command registry, generated clients, canonical/runtime OpenAPI, Tauri, and MCP advertised surfaces are exact at 53 commands.
- Lease/blob/manifest/runtime infrastructure remains REST-only because owner tokens and binary payloads must not cross MCP. High-level user operations remain the MCP boundary.

## Commands and results

- `cargo test -p buglogin-api` — pass: all unit, HTTP, registry, compatibility, MCP, operation/event, and doc tests.
- `cargo test -p buglogin-api --test http_sync_migration` — pass: genesis activation, replay, changed-input conflict, wrong-base preservation, destination fencing remap, Redis manifest non-commit.
- `cargo test -p buglogin-api ambiguous_finalize_is_recovered_from_durable_receipt_without_rollback` — pass.`r`n- Recovery unit coverage — pass: two-pass `Finalizing → CleanupPending → Abandoned`, referenced-object preservation, delete-failure preservation, idempotent replay.
- `cargo test -p buglogin-sync --test export_import` — 7/7 pass.
- `cargo test -p buglogin-migration --test sync_package` — 2/2 pass.
- `node --test tests/contracts/contract-drift.test.mjs` — 6/6 pass.
- `node scripts/ci/check-contracts.mjs` — pass: 59 capabilities, 50 work packets, 53 commands.
- `cargo test -p buglogin-api --test http_package_upload` — 2/2 pass: CAS upload, signature verify, replay, immutability, wrong-key rejection.
- `cargo test -p buglogin-api --test http_signed_sync_package` — pass: profile-bearing signed control+sync upload, verify, hosted managed-profile/binding activation without local repository writes, durable aggregate proof, exact replay, proven sync-failure control rollback, same-idempotency reactivation, Redis manifest non-commit.
- `node scripts/ci/check-secret-boundaries.mjs` — pass.
- `pnpm check:api` — generated artifacts current.
- `cargo fmt --all -- --check` — pass.
- `cargo test -p buglogin-api stale_finalizing_sync_journal_recovers_after_restart_idempotently` — pass.
- `cargo test -p buglogin-api recovery_delete_failure_preserves_finalizing_journal_and_object` — pass.
- `cargo test -p buglogin-browser disabled_profile_service_never_opens_local_authority` — pass.
- `cargo test -p buglogin-browser --test platform_windows -- --test-threads=5` — pass; repeated 10/10 parallel stress runs.
- `cargo test -p buglogin-browser` — pass with default parallel execution.

Rust build target used `D:\buglogin-build` to avoid filling the repository volume. No Docker service was started.

## Remaining release blockers

1. MCP-safe high-level migration command over the shared import service; raw binary upload and lease secrets remain REST-only.
2. Extend durable recovery coverage to package-upload journals and add orphan reference-scan/GC for crash-time `Staging` blobs.
3. Orphan-blob retention/GC with reference scan, two-pass confirmation, and checksum-CAS delete.
4. Real PostgreSQL/Redis/S3-compatible failure rehearsal, including unknown PostgreSQL commit result through a fresh connection.
5. Streamed package verification; the current verified-package reconstruction has a 512 MiB in-memory ceiling.
6. Remaining 20 migration fixtures and production source snapshot/export evidence.
7. Windows and macOS signed build plus migration/launch/proxy/cookie/sync E2E evidence.

## Decision

Keep the production/local deployment split and keep the package false-success guard. Hosted sync transaction core is implemented and tested; production migration remains blocked.


# Hosted Signed Package Ingress Evidence — 2026-07-22

## Implemented

- `PUT /v1/migrations/packages/{packageId}/files?path=...`
  - bearer authentication and `migration.write`
  - required idempotency key and SHA-256 header
  - normalized relative package paths
  - 64 MiB per-file limit
  - object-store checksum CAS
  - durable PostgreSQL-compatible upload journal and audit
  - exact replay and post-verification immutability
- `POST /v1/migrations/packages/{packageId}/verify`
  - reconstructs the staged neutral package from object storage
  - verifies checksum coverage, manifest, secret policy, and HMAC signature
  - binds manifest package ID to the upload ID
  - durable verified state
- `POST /v1/migrations/packages/{packageId}/activate`
  - re-verifies the signed package
  - imports control metadata through the shared Rust control importer
  - imports optional `sync/export.json` through the shared hosted sync transaction core
  - rolls back control import on a proven sync failure
  - never rolls back an ambiguous sync commit outcome
  - durable composite activation receipt and exact replay

## Evidence

```text
cargo test -p buglogin-api --test http_package_upload
2 passed

cargo test -p buglogin-api --test http_signed_sync_package
1 passed: success, exact replay, sync failure rollback, same-key retry

cargo test -p buglogin-api
all unit/integration/doc tests passed

node scripts/ci/check-contracts.mjs
PASS: 59 capabilities, 50 work packets, 53 commands

node --test tests/contracts/contract-drift.test.mjs
6 passed

cargo fmt --all -- --check
pass
```

## Still Blocking Production Migration

- Hosted profile metadata activation now writes the durable control aggregate; profile-resource bindings no longer depend on the local profile repository.
- Profile-bearing signed control+sync package E2E derives canonical mapped IDs, activates the managed profile and binding, verifies durable `syncMode=managed`, and proves the local profile repository remains empty.
- Signed control+sync package E2E passes, including exact activation replay and Redis manifest non-commit.
- Proven sync failure now demonstrates control rollback plus successful same-idempotency reactivation in package E2E.
- Sync-journal startup recovery now handles stale proven-uncommitted finalization/cleanup states idempotently; package-upload journal recovery and staging orphan GC remain open.
- Verification currently reconstructs at most 64 MiB in memory. Keep the aggregate package cap at this value on the current VPS; implement streamed verification before raising it for larger migrations.
- Orphan object GC, real PostgreSQL/Redis/S3 rehearsal, remaining migration fixtures, and Windows/macOS signed E2E remain open.
- The local filesystem `/v1/migrations/import` guard for `sync/export.json` remains intentional; production uses hosted package ingress only.

- Package-file ingress now persists the immutable journal intent before remote object write. A failed first blob write leaves a recoverable journal entry; exact retry repairs the missing checksum-bound object instead of creating an untracked orphan. `package_upload_retry_repairs_journaled_missing_blob` passes.


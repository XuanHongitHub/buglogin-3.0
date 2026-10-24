# Profile error reporting and BugLogin 3.0.7 release

## Status

Implementation and local gates are complete. No stable feed or public release is changed until the report path, release source, signed platform artifacts, and required device evidence are verified.

## Phases

1. Add an asynchronous, redacted, idempotent report for failed profile launches using the existing authenticated API feedback contract; add focused regression coverage.
2. Reproduce the `Live object conflicts with tombstone` path, identify whether this is a safe sync conflict or a manifest construction bug, and fix only with data-preserving evidence. **Done:** snapshot retained a 30-day tombstone when the browser recreated that path; the resulting manifest was invalid before upload. Snapshots now omit tombstones for currently live paths, and a newer lease fence is required to retire a retained tombstone.
3. Run focused Rust/API/release checks; verify the production endpoint accepts the current contract without creating a record.
4. Reconcile the complete 3.0.7 source across Malvanut product `main` and XuanHong Actions `main`, with clean immutable commits and matching release-critical files.
5. Build and sign the Windows/macOS candidate from the Actions repository. Verify signatures, notarization/stapling, updater metadata, SBOM/provenance, checksums, and S3 public readback.
6. Install/update smoke the exact artifacts on supported OSes. Publish stable only when required Windows and macOS evidence exists; otherwise retain the verified candidate and report the exact gate.

## Verification record

- Focused report/API tests: profile launch diagnostic unit test 1/1; feedback persistence/idempotency API test 1/1; profile-viewer API tests 3/3. Production no-auth validation probe returned 422; no authenticated production feedback record was created or verified.
- Sync-conflict regression: sync crate 78 tests passed, including retained-tombstone fencing tests; desktop projection recreation test 1/1; Windows-target desktop library `cargo check` passed; `cargo fmt --all -- --check` passed.
- Release checks: contract 36/36, release verifier 23/23, web tests/typecheck and desktop tests/typecheck passed before the final sync patch.
- Source synchronization and protected workflow: pending.
- Windows/macOS install/update evidence: pending.
- Stable S3 publication/readback: pending.

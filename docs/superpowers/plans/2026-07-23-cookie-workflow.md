# Cookie Workflow Plan

1. [x] Persist metadata and vault versions transactionally
2. [x] Materialize real Wayfern cookie database
3. [ ] Add list/import/copy contracts and ACL
4. [ ] Wire REST/Tauri/MCP with redaction
5. [ ] Add desktop cookie workflow states
6. [ ] Verify sync and migration cohorts

## Gate

Do not mark `CAP-COOK-001` complete from registry/routes alone. Required evidence: restart persistence, batch rollback, stopped-profile enforcement, Wayfern launch session, Windows Credential Manager, macOS Keychain, sync interruption/recovery, migration `MIG-003`, and zero-secret diagnostics scan.

## Persistence evidence — 2026-07-23

- Append-only atomic metadata revisions survive service restart.
- Cookie values remain OS-vault-only; persisted ledgers contain metadata, slot, and vault version only.
- Immutable two-slot secret staging preserves the active cookie when metadata commit fails.
- Multi-cookie commit failure removes every staged secret before returning.
- `cargo test -p buglogin-browser` passes, including restart and rollback regressions.

## Wayfern materialization evidence — 2026-07-23

- Chromium schema 23 and platform-specific `Default/Cookies` path follow the pinned DonutBrowser upstream implementation.
- SQLite writes are transaction-atomic, reject running profiles, and metadata reads never expose values or create missing databases.
- Desktop/API command wiring remains pending. Hosted REST cannot directly decrypt client-owned snapshots or access native profile files; routing requires an approved connected-device job contract, not a server-side cookie ledger.

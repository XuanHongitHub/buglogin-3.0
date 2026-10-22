# Cookie Workflow Plan

1. [x] Persist metadata and vault versions transactionally
2. [ ] Materialize real Wayfern cookie database with runtime-compatible encryption
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

- Chromium schema 23 and platform-specific `Default/Cookies` path are implemented, and SQLite writes are transaction-atomic.
- The current implementation rejects plaintext persistence and accepts only pre-encrypted Chromium `v10`/`v11` values. It does not yet obtain the matching Wayfern encryption password or encrypt values for that runtime, so cookie mutations are not materialized into the browser database.
- The pinned Donut reference writer stores plaintext in `cookies.value`; that path is not acceptable for BugLogin and cannot be used as implementation evidence.
- Hosted REST has a connected-device job contract, but desktop polling/execution and native encrypted commit remain incomplete. Keep `CAP-COOK-001` open until runtime-key provisioning, encrypted materialization, and device execution are implemented.

## Authority restore hardening — 2026-10-16

- Persisted cookie stage IDs are validated for uniqueness before stage secrets are restored; duplicate IDs fail closed as a storage error instead of overwriting an earlier stage. New pending materialization records persist the fencing token and import stage reference so restore can recompute and compare the receipt fingerprint, including copy source/destination binding. Older records remain readable through the prior structural validation path.
- This is source-only until the consolidated verification pass. Hosted device binding, authenticated polling/claim/lease/receipt, and end-to-end materialization evidence remain open.

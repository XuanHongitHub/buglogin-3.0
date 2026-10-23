# BugLogin 2.0 Full-App Readiness Wave

Status: controlled implementation; production release blocked.

## Completed in this wave

- [x] Canonical command registry regenerated after adding Tauri adapters for profile clone/share/revoke.
- [x] Desktop bridge now invokes those commands through Tauri; Rust fail-closed production routing remains authoritative.
- [x] API, browser, desktop, contract, and migration targeted gates rerun.
- [x] Local cookie authority and Tauri adapter cover list/stage/import/copy, replay, stale CAS, restart, and lock semantics.
- [x] Synthetic migration rehearsal covers activation, quarantine-preserving rollback, repeated rollback, and rollback retry.
- [x] Web audit/incidents routes are reachable and show explicit contract blockers without fabricated records.
- [x] Donut reference files verified byte-identical for shared clone dialog, profile table, and global theme CSS.
- [x] `.tmp/` and legacy production roots untouched.

## Remaining P0 gates

- [x] Local REST/MCP expose metadata-only `cookie.list`; cookie mutations fail closed with an explicit connected-device dispatch error. Local mutation authority remains Tauri/device-bound.
- [x] Hosted cookie REST connected-device job queue, device inbox/claim/ack, and metadata receipt path implemented in source; cookie value operations remain Tauri/device-local, and MCP intentionally does not expose them. Consolidated code verification and native Wayfern materialization evidence remain open in `docs/superpowers/plans/2026-10-13-connected-cookie-job-boundary.md`.
- [ ] Native Windows/macOS vault evidence for cookie authority.
- [ ] Desktop clone/share/revoke/cookie UI with screenshot, keyboard, a11y, and state evidence.
- [x] Full legacy control-plane export and one-pass direct clone: see `docs/superpowers/plans/2026-10-16-legacy-cloud-profile-migration.md` and its source-to-target evidence.
- [x] 1.0 profile-payload import waived by owner on 2026-10-17 for the initial 3.0 launch. Existing cloned profile metadata and staged payloads remain preserved; no old data was deleted or imported by this waiver.
- [ ] Verified Bugmini artifact and native Windows evidence.
- [ ] macOS arm64 and Intel/waiver evidence.
- [ ] Trusted-TLS PostgreSQL/S3/Redis interruption and recovery evidence.
- [ ] Signed/notarized release artifacts, SBOM, provenance, updater rollback.

## Release rule

Do not claim `ready`, run production cutover, or touch the legacy/production data roots until every remaining P0 gate has evidence recorded under `docs/e2e/`, `docs/migration/`, and `docs/release/`.

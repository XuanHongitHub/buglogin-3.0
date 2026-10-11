# Profile and Proxy Unified Sync Plan

Status: Rust API, metadata dependency validation, desktop API transport, client-side profile-file encryption/decryption, and a profile-scoped recovery drawer are implemented locally. The desktop persists each profile/resource key and its recovery key in the OS vault, adds a resource-scoped encrypted key envelope to each synced manifest, validates recovery through authenticated Tauri commands, and keeps envelopes/proxy metadata out of browser profile projection. Proxy credential envelopes bind workspace, profile resource, proxy ID, and revision; the desktop proxy editor now routes credentials through a revision-scoped OS-vault boundary, API mutations receive only the credential-present marker, and managed launch/snapshot require a matching proxy revision. Targeted Rust/API/desktop/contract gates pass locally. Production hosted sync remains blocked by the unverified staging contract, secure resource-sharing key distribution, durable recovery audit and visual QA, incremental/resumable transfer, and Windows/macOS release evidence.

## Phase 0 — verify authority

- [x] Confirm `Profile.proxy_id` is the existing local assignment authority.
- [x] Confirm the current app still has a per-proxy `sync_enabled` field and command/UI toggle.
- [x] Record that the current hosted launch/sync contract is draft and lacks proxy CRUD, proxy-secret envelopes, and atomic profile-plus-proxy commits.
- [ ] Verify `api.buglogin.com` OpenAPI contract and staging behavior with owner-approved credentials. Read-only probe on 2026-10-12: `/health` returned `200 {"status":"ok"}` and `/readyz` returned `200 {"status":"ready","s3":true}`, but `/v1/health`, `/openapi.json`, `/v1/openapi.json`, `/api/openapi.json`, and `/.well-known/openapi.json` returned `404`. The host is reachable, but the deployed routes do not prove compatibility with this repository's v1 contract; no authenticated/staging write was attempted.
- [x] Add exact-origin credentialed CORS and browser preflight support to the Rust API; verify with cross-origin router tests.
- [x] Align API and desktop single-object sync limits at 64 MiB; enforce the limit before upload side effects and publish it in OpenAPI.
- [ ] Approve versioned workspace/ACL, profile, proxy, secret-envelope, sync-manifest, chunk, conflict, and quota contracts.
- [x] Define the draft manifest dependency shape and identify why a mutable proxy revision alone cannot provide atomic sync.
- [x] Add a backward-readable Rust manifest dependency model that binds proxy metadata/envelope references to checksummed manifest objects.

## Phase 1 — server authority

- [x] Implement Rust API services for profile/proxy CRUD, binding, authorization, audit, idempotency, and typed errors.
- [ ] Implement atomic manifest activation including immutable proxy metadata and encrypted-envelope object references plus only changed profile data chunks.
- [x] Add API validation that the manifest dependency set matches the selected proxy, current revision, same-workspace read grant, and canonical safe metadata object under the serialized manifest mutation.
- [x] Add a versioned manifest encryption declaration; reject raw profile blob uploads and non-empty commits without a resource-scoped key envelope and supported AES-GCM chunk wire format.
- [x] Reject manifest reads when the selected proxy revision has changed since the active generation, so clients cannot treat stale profile/proxy state as current.
- [ ] Implement encrypted credential-envelope lifecycle and workspace/device key rotation and revocation.
- [x] Keep REST/OpenAPI/MCP coverage aligned with the canonical API command and resource contracts; local API and sync contract suites pass.
- [ ] Add staging integration tests; prove retries, conflicts, partial transfers, and outages preserve the prior active generation.

## Phase 2 — desktop sync engine

- [x] Use one Rust BugLogin API client path for profile and proxy metadata/data; production mode must not contact Donut services or fall back to local authority.
- [x] Fix encrypted chunk object-key collisions caused by random AES-GCM nonces: new remote keys include the ciphertext checksum while authenticated data remains bound to the stable logical chunk key; old manifest keys remain readable.
- [ ] Add incremental sync without weakening the current manifest authority: version the payload/object-reference contract; preserve v1 reads; permit reuse only for an exact object already committed to the same resource and resource-key version; then connect the existing chunk utility to file hashing, resume, bounded concurrency, coalescing, and reconciliation.
- [ ] Sync a proxy only as a dependency of an explicitly synced profile; send one shared proxy payload once per revision/hash.
- [ ] Keep proxy secrets in the OS vault and transfer only encrypted envelopes; prove missing/revoked keys fail closed with actionable errors.
- [x] Define a v1 proxy credential envelope primitive with profile-resource and proxy-revision authenticated scope.
- [x] Encrypt profile files before upload and decrypt only after download using per-resource keys stored in the OS vault; bind associated data to workspace, resource, path, and generation.
- [x] Add an authenticated recovery-code read and key recovery command; keep recovery secrets out of generic stop/idempotency results.
- [x] Add the user-facing recovery UI and profile-scoped key backup/recovery actions.
- [ ] Add durable audit evidence and verified key distribution/rotation/revocation for shared profiles.
- [ ] Migrate old proxy sync flags without dropping local proxy data; produce an auditable report and rollback path.

## Phase 3 — UI and release evidence

- [ ] Remove independent proxy sync toggles after backend/API behavior is deployed and migration passes.
- [ ] Show profile sync state and its proxy dependency state from Rust/API results, including offline, conflict, quota, locked, and missing-secret cases.
- [ ] Run Rust/API/contract tests, Windows and macOS vault evidence, interruption/conflict E2E, performance measurements, and screenshot/keyboard/accessibility QA.
- [ ] Record release evidence and rollback procedure before production rollout.

## Stop conditions

- No verified BugLogin staging contract/credentials: stop before API writes or endpoint wiring.
- No secret-envelope/key lifecycle: stop proxy credential synchronization.
- No atomic manifest support: do not remove the current proxy toggle or claim unified production sync.
- Any data-loss, cross-workspace reference, credential leakage, or failed rollback evidence blocks release.

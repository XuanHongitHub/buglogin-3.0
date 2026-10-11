# Profile and Proxy Unified Sync Plan

Status: Rust API, metadata dependency validation, and the desktop BugLogin API transport are implemented locally; production hosted sync remains blocked on secret-envelope lifecycle, staging verification, incremental transfer, and release evidence.

## Phase 0 — verify authority

- [x] Confirm `Profile.proxy_id` is the existing local assignment authority.
- [x] Confirm the current app still has a per-proxy `sync_enabled` field and command/UI toggle.
- [x] Record that the current hosted launch/sync contract is draft and lacks proxy CRUD, proxy-secret envelopes, and atomic profile-plus-proxy commits.
- [ ] Verify `api.buglogin.com` OpenAPI contract and staging behavior with owner-approved credentials.
- [ ] Approve versioned workspace/ACL, profile, proxy, secret-envelope, sync-manifest, chunk, conflict, and quota contracts.
- [x] Define the draft manifest dependency shape and identify why a mutable proxy revision alone cannot provide atomic sync.
- [x] Add a backward-readable Rust manifest dependency model that binds proxy metadata/envelope references to checksummed manifest objects.

## Phase 1 — server authority

- [x] Implement Rust API services for profile/proxy CRUD, binding, authorization, audit, idempotency, and typed errors.
- [ ] Implement atomic manifest activation including immutable proxy metadata and encrypted-envelope object references plus only changed profile data chunks.
- [x] Add API validation that the manifest dependency set matches the selected proxy, current revision, same-workspace read grant, and canonical safe metadata object under the serialized manifest mutation.
- [ ] Implement encrypted credential-envelope lifecycle and workspace/device key rotation and revocation.
- [x] Keep REST/OpenAPI/MCP coverage aligned with the canonical API command and resource contracts; local API and sync contract suites pass.
- [ ] Add staging integration tests; prove retries, conflicts, partial transfers, and outages preserve the prior active generation.

## Phase 2 — desktop sync engine

- [x] Use one Rust BugLogin API client path for profile and proxy metadata/data; production mode must not contact Donut services or fall back to local authority.
- [x] Fix encrypted chunk object-key collisions caused by random AES-GCM nonces: new remote keys include the ciphertext checksum while authenticated data remains bound to the stable logical chunk key; old manifest keys remain readable.
- [ ] Add incremental change tracking, content hashing, chunk resume, bounded concurrency, coalescing, size limits, and reconciliation.
- [ ] Sync a proxy only as a dependency of an explicitly synced profile; send one shared proxy payload once per revision/hash.
- [ ] Keep proxy secrets in the OS vault and transfer only encrypted envelopes; prove missing/revoked keys fail closed with actionable errors.
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

# Encrypted Group Profile Sharing Plan

## Status

Contract finalized for implementation. Encrypted group sharing remains blocked until every stage and the authenticated staging journey pass. No production cutover is implied.

## Stage 1 — one effective recipient resolver (IO-CONTROL)

- [x] Add a Rust-owned resolver for direct and current group `read`/`execute` recipients, active workspace memberships, direct/group denies, and explicit profile capability denies; rotation paths bind active devices and keys.
- [x] Reuse the resolver in encrypted authorization, rotation recipient listing, envelope validation, and manifest compatibility checks.
- [x] Keep unencrypted dynamic group authorization behavior unchanged.
- [ ] Add focused unit/API coverage for union, deny precedence, moves, inactive members/devices, and cross-workspace boundaries.

## Stage 2 — atomic ACL fences (IO-CONTROL)

- [x] Direct resource-grant create/revoke through REST and MCP for `read`/`execute` on encrypted profile resources sets the rotation fence inside the same control mutation transaction; regression source added for the shared service helper, not run.
- [x] Group grant create/revoke and profile group assignment changes identify affected encrypted profiles before hosted mutation persistence.
- [x] Re-evaluate the resulting encrypted-profile recipient set before setting group-change/member-activation fences and accepting device-key enrollment; reject over-limit changes exceeding the 256-member or 64-device-per-member rotation limits. Regression/build verification remains deferred.
- [x] Member activation/suspension and new device-key enrollment identify affected encrypted profiles; revoking a device-bound session now fences resources with a current envelope before session revocation. Regression source covers identifying affected profiles; test execution remains deferred.
- [x] Persist group/profile ACL and rotation-pending fences in the same hosted control aggregate mutation; session revocation uses safe fence-before-revoke ordering across identity/control persistence.
- [x] Enforce bounded profile fan-out with fail-closed semantics; mutations do not partially publish control state when the limit is exceeded.
- [ ] Verify failure rollback, idempotent replay, concurrent mutations, and persistence reopen in the deferred consolidated pass.

## Stage 3 — exact envelope rotation (IO-DATA)

- [x] Extend owner rotation snapshots to include effective group recipients and bound device-key revisions/fingerprints.
- [x] Revalidate the exact recipient/device set and current key version at commit.
- [x] Atomically persist the re-encrypted manifest, complete envelope set, audit/replay result, removal of stale envelopes, and fence clear.
- [ ] Run coverage for missing/extra/stale/duplicate/wrong-scope/wrong-fingerprint envelopes and former-recipient denial during the consolidated verification pass.
- [ ] Prove REST and MCP encrypted manifest reads for active direct/group recipients with a current device envelope; deny missing/stale envelopes and former recipients; verify the rotation-pending owner-only read rule in the consolidated pass.
- [x] Fail closed when an active effective recipient has no active device or complete registered key coverage; regression source added, execution deferred.

## Stage 4 — desktop owner flow (IO-UI)

- [ ] Surface the typed `profile_key_rotation_required` state from launch and provide a recipient-review action on DES-01.
- [x] Fetch the typed effective-recipient/device-key snapshot through Tauri; expose an idempotent key-rotation operation input for exact member/device/fingerprint confirmation.
- [x] Bind backend confirmation to the reviewed snapshot and reject missing, duplicate, malformed, or stale recipient/device fingerprints before stopping or syncing.
- [x] Add the idle runtime sync-only lease primitive and desktop path: hydrate the encrypted generation without starting a browser, acquire an update lease, recheck the manifest, and let the runtime heartbeat renew through commit/cleanup.
- [x] Keep lease permission selection consistent between acquire/renew during key rotation; reject logically expired renewals across local and Redis coordination and align Redis key TTL to `expiresAt`. The current-source selection/expiry unit tests pass; the opt-in live Redis regression is added but was not run because no Redis test endpoint is configured.
- [ ] Add the owner recipient-review action and confirmation UI on DES-01 using an approved Donut reference package.
- [x] Rotate resource keys, re-encrypt the full profile generation, and seal replacement device envelopes from Rust-held OS-vault keys. The desktop sync path stages replacement keys, encrypts the successor snapshot, commits manifest plus exact recipient envelopes atomically, then activates the staged key. This is implemented in source; consolidated verification and native-vault recovery evidence remain open.
- [ ] Show actionable offline/missing-key/stale-snapshot errors using approved Donut patterns; no simulated progress or fake recipient state.
- [ ] Add Rust/desktop integration tests for idle/running execution, retry, crash, lease renewal/release, and key-vault recovery.

## Stage 5 — migration and release evidence (IO-AUTO/IO-QA)

- [x] Preserve control-only imported group grants and atomically fence affected encrypted resources until owner-confirmed rekey supplies complete device envelopes.
- 2026-10-16 source follow-up: import now compares pre-import and merged grants/profile assignments, so partial imports account for pre-existing grants without re-fencing unchanged grants on package replay; deny and revoked `read`/`execute` grants conservatively fence matching managed resources. Regression source covers denial, revocation, multiple group assignments, and unchanged replay. This follow-up has not been tested.
- [ ] Add API/OpenAPI/generated-client parity and MCP/Tauri authority checks for all changed operations.
- [ ] Add authenticated staging E2E for grant, group reassignment, sync while pending, owner rekey, recipient sync, revoke, and former-recipient denial.
- [ ] Reconcile the migration graph during the single production-data diff/import pass, retain its rollback checkpoint, and record backup/restore evidence.
- [ ] Re-run desktop and API targeted gates and platform release evidence without a full local/release build unless the release lane requires it.

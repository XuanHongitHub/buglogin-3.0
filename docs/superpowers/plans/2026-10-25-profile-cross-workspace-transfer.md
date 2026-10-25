# Canonical cross-workspace profile transfer plan

Status: design/proposal only; no implementation authorized by this document.

## Goal and boundary

Implement a true, auditable move of a profile between workspaces (including explicitly authorized cross-organization moves), with encryption re-binding and source revocation. This is separate from the already-supported group assignment/no-group operation. Do not change archive/export/import semantics as a shortcut.

## Phases

### 0. Contract freeze and ownership

- [ ] Review this design with IO-CONTROL, IO-DATA, IO-AUTO, IO-CORE, IO-UI and IO-QA; resolve stable-ID behavior against actual persistence uniqueness.
- [ ] Freeze command names, state machine, DTOs, error taxonomy, source/destination authority, grant defaults, proxy mapping, audit fields, quotas and retention.
- [ ] Add capability/command ownership rows and dependency ordering before parallel implementation.
- Gate: no duplicate transfer authority, route-owned business logic, frontend-only ACL, or dependency on copying source envelopes.

### 1. Domain and authorization contract (IO-CONTROL)

- [ ] Add typed transfer ID/state/receipt and dual-scope policy evaluation.
- [ ] Add durable begin/commit/cancel/replay service with expected revisions and strict state transitions.
- [ ] Fence source lifecycle, sync, ACL, archive/delete, and group/proxy changes while reserved.
- [ ] Commit source tombstone, destination binding, group, grants, envelope references, revisions, audit and replay receipt atomically.
- Gate: role matrix covers platform, same-org owner, unrelated org, source-only, destination-only, workspace owner/admin/lead and explicit denies.

### 2. Data and cryptography (IO-DATA, coordinated with IO-CORE)

- [ ] Specify staged object namespace, deterministic transfer journal receipts, bounded resumability and cleanup ownership.
- [ ] Implement connected-device prepare: source integrity verification, fresh destination key, full logical data re-encryption, envelope generation and exact destination-recipient snapshot.
- [ ] Implement explicit proxy map/remove and local credential open/reseal using supported key authority.
- [ ] Validate manifest, references, checksums, quota, resource/key scope, recipient/device set and proxy revisions before commit.
- [ ] Add restart recovery at every state and orphan cleanup that proves no committed manifest references a candidate object.
- Gate: server never sees plaintext; one active binding; old source key/envelopes cannot authorize destination; failure never publishes partial data.

### 3. Transport and parity (IO-AUTO)

- [ ] Add REST preflight/begin/status/device-dispatch/commit/cancel endpoints calling the shared service.
- [ ] Register canonical command metadata and update OpenAPI, adapter contract, MCP schemas and generated clients.
- [ ] Implement MCP dispatch to an authorized connected device; reject direct key/plaintext input and require a device-prepared receipt to commit.
- Gate: REST/MCP idempotency, authorization, audit, response and error parity; no unregistered side channel.

### 4. Desktop orchestration and UI (IO-CORE then IO-UI)

- [ ] Tauri stops and syncs source, checks lease release, accesses only OS-vault material, stages encrypted output, and supports restart/resume/cancel.
- [ ] Build a full-page transfer flow using Donut patterns and approved screen state matrix; show source/destination scope, preflight blockers, group/No Group, grants, proxy map/remove, exact device fingerprints, size/progress, conflicts and receipt.
- [ ] Keep action disabled with server-derived reasons when source runs, leases, dirty sync, missing device/key, inaccessible destination or unsupported dependency blocks transfer.
- Gate: keyboard/focus, loading/offline/forbidden/conflict/interrupted/cleanup states, screenshot evidence; no modal or local optimistic commit.

### 5. Verification and staged rollout (IO-QA)

- [ ] Unit/property tests for AAD scope mismatch, data re-encryption, checksum preservation, proxy mapping, recipient freshness, no-group, group move and grant clearing.
- [ ] API persistence tests for dual-scope ACL, expected revision, idempotency/replay, changed-input conflict, crash/reopen and audit redaction.
- [ ] Fault injection after reserve, every object upload, staged manifest, pre-commit, commit response loss and cleanup; assert state/data invariants.
- [ ] E2E same organization and cross-organization; source/destination users/devices; encrypted/unencrypted profile; cookies/storage; proxy credentials; sync before/after; source denied and destination allowed.
- [ ] Verify offline source devices cannot publish stale writes after fencing and old recipients cannot fetch new-generation envelopes.
- [ ] Run Windows and macOS install/runtime/transfer smoke with physical or approved VM evidence.
- [ ] Stage with isolated test orgs and object prefix; inspect audit, orphan cleanup, quotas and retention before any production enablement.
- Gate: no production migration or release until two interruption/recovery rehearsals, authorization review, source-access denial, destination usability and both OS evidence pass.

## Non-goals for this plan

- Group reassignment within one workspace, bulk moves, profile duplication, account/member transfer, org ownership transfer, UI redesign outside the transfer page, changing crypto algorithms, or deleting historical source ciphertext immediately.

## Definition of done

The canonical resource moves exactly once; source and destination authority are correct; all contents verify under a fresh destination key; no source grant/envelope remains active; destination group/grants/proxy choices match the confirmed plan; retries and restarts are deterministic; REST/Tauri/MCP share one Rust-owned contract; and Windows/macOS end-to-end evidence plus the residual-risk record are complete.

# Canonical profile transfer across workspaces and organizations

## Objective

Provide an explicit move of a canonical profile resource from one workspace to another, including cross-organization moves when the actor has authority over both scopes. This is a separate operation from assigning/removing groups or copying cookies.

## Existing constraints traced in source

- `profile.groups.replace` is workspace-scoped; an empty group list means `No Group`. This remains the in-workspace organization action.
- `ResourceKeyEnvelope` key wrapping uses `buglogin/resource-key/v1/{workspace_id}/{resource_id}` AAD (`crates/sync/src/crypto/mod.rs`). A source envelope cannot be replayed for a destination scope.
- Device key-share envelopes use `buglogin/profile-key-share/v2/{workspace_id}/{resource_id}/{device_id}/{key_version}` AAD.
- Proxy credential envelopes use `buglogin/proxy-credential/v1/{workspace_id}/{resource_id}/{proxy_id}/{proxy_revision}` AAD. The existing manifest validates proxy dependency/envelope consistency.
- Encrypted chunks are authenticated against their deterministic logical object path in `crates/sync/src/chunks/mod.rs`; the manifest binds the object set and per-resource key envelope. A complete destination generation must be decrypted/verified and re-encrypted under a fresh resource key; copying only manifest metadata or envelopes is invalid.
- The server must remain blind to profile bytes, resource keys, private device keys, cookies and proxy credential plaintext.
- OpenSpec encrypted sharing already requires an exact, current destination recipient/device set and a rotation-pending fence when access changes.

## Product semantics

```text
Source Org / Workspace                     Destination Org / Workspace
  Profile (active)      -- transfer -->     Profile (same logical identity)
  old group/grants                         chosen Group OR No Group
  old envelopes                            explicit destination grants/envelopes
  old manifest                             fresh encrypted generation
  source binding becomes tombstone         exactly one active binding
```

- Transfer is a **move**, not a copy. The source is fenced before cryptographic preparation. After commit, the source binding is a transfer tombstone and destination is the only active binding.
- Group assignment within the current workspace continues to use `profile.groups.replace`; transfer never disguises a workspace change as a group edit.
- Keep profile/resource IDs stable if existing uniqueness/persistence constraints allow. The transfer contract must be finalized against storage uniqueness before implementation; if IDs must change, record an explicit source-to-destination identity mapping in the durable receipt and audit trail.
- Destination group is explicit and must belong to the destination workspace; `No Group` is an explicit supported selection.
- No source grants or group-derived access are copied. The plan shows source access removal and destination access set. Destination grants are explicit; actor retains a valid destination management path. Encrypted access requires complete envelopes for the exact active devices resolved by the shared Rust recipient service.
- Cross-org authority requires Platform Admin or effective explicit authority over both source and destination scopes. An org owner cannot transfer into an unrelated organization by virtue of ownership of the source.

## Encrypted data and AAD

The connected desktop is the cryptographic worker:

1. Stop the source profile, flush its stopped projection, and prove no runtime lease remains. The API refuses begin/prepare when the source is running or has an active lease.
2. Retrieve the authorized source manifest/objects and source key material only through the current device-bound authorization and OS vault path.
3. Verify each source object authentication tag, plaintext size/checksum, manifest generation, and dependency. Abort before commit on any mismatch.
4. Generate a fresh destination resource key. Decrypt/re-encrypt every logical file/chunk, preserving logical profile contents but producing a new complete generation. This prevents destination access from depending on old source recipients' copied resource keys.
5. Create resource-key envelopes using destination workspace/resource AAD for the exact confirmed destination devices. Re-resolve the recipient set at commit; stale/incomplete/extra/over-limit sets fail.
6. Resolve proxy dependencies explicitly. For each dependency, choose a compatible destination proxy or `Remove dependency`. If retained, the desktop opens the credential through the existing credential-key authority, then reseals it using destination workspace/resource/proxy/revision AAD. The server never translates credentials. Missing or stale credential authority blocks retention.
7. Validate destination manifest, full object set, checksums, group scope, proxy revision and all envelopes before destination publication.
8. After commit, persist the destination key only in the OS vault under the destination scope. If the app exits after server commit but before this local write, recover the key from the new device-bound destination envelope; never fall back to the source key or an unscoped cache entry. Zeroize temporary plaintext/key buffers where supported.

The manifest/chunk AAD is logical-path-bound, but the resource-key and device envelopes are scope-bound. Fresh-key re-encryption is required for a true move with source-access revocation; simply copying chunk bytes or rewrapping the old key is not acceptable.

## Authorization and lifecycle

- Check separately: source `profile.transfer`/source removal authority, destination resource create/manage authority, and destination group/proxy/grant authority.
- Enforce effective grants and deny precedence, active memberships, organization/workspace boundaries, token scopes, expected revisions, current profile binding, and actor identity in Rust/API. UI checks are advisory only.
- Reject archived, running, actively leased, already-transferring, conflicted, rotation-pending, or unbound profiles. The runtime must be stopped and sync current before begin.
- Preflight also checks destination quota, max recipient/device limits, destination group, explicit grant set, proxy map, source key availability, connected device, estimated bytes, and supported encryption versions.
- Transfer entry fences source launch, writes, sync, archive/delete, and conflicting ACL/group mutations. ACL/device changes on the destination invalidate recipient preparation.

## Durable protocol and atomicity

Object storage cannot participate in the Postgres/control-store transaction. Use a durable journal and a single authoritative control commit:

```text
preflight -> reserved/source-fenced -> preparing on authorized device
          -> staged and verified -> atomic control commit -> committed
                               \-> cancel -> cleanup-pending -> canceled
```

1. `preflight` is read-only and returns source/destination revisions, blockers, recipients, proxy choices and impact summary.
2. `begin` checks source+destination scope, expected revisions and idempotency; persists the journal and source fence atomically. No destination resource is visible.
3. The authorized device streams encrypted, content-addressed destination objects to staging with bounded concurrency/retries. Each stage receipt is journaled. The staged manifest remains private/unpublished.
4. `commit` rechecks source fence/revisions, destination ownership/quota, group/proxy scopes, exact recipients/fingerprints, manifest/object checksums and all ACL state under the control mutation lock.
5. One durable control transaction publishes destination binding and manifest pointer, new envelopes, destination grants/group, source transfer tombstone and source grant/envelope removal, increments relevant revisions/fences, records audit and idempotency receipt, and marks journal committed. This is the only visibility point.
6. Before commit, any failure leaves the source authoritative but fenced until deterministic resume or cancel. Cancel marks staged objects cleanup-pending; cleanup verifies no committed manifest references them before deletion. A retry with the same input resumes idempotently.
7. After commit, retain source ciphertext under existing retention/GC policy. Do not automatically roll back by restoring the old manifest, key, envelopes or grants. Reversal is a new transfer with current authorization and fresh encryption.

Exactly one active binding is an invariant at every recoverable state. A lost response after commit returns the original receipt on replay. No ambiguous “success” inference from missing source records is allowed.

## Proxy, groups, grants, and audit

- A destination group must be in the target workspace. `No Group` writes an empty assignment set.
- Remove all source direct grants, source group assignments and source envelopes at commit. Retire group-derived access immediately when source is fenced; never carry source group grants to another workspace.
- Destination access is explicit. Show effective recipients and all device fingerprints before encrypted transfer; commit only the exact set. If a destination recipient/device changes after confirmation, require re-preparation.
- Every source proxy dependency requires a destination proxy mapping or explicit removal. Validate proxy type/revision/policy. Never copy source proxy IDs or credential envelopes across scope. Retained credentials are opened/resealed on device; removal strips dependency and envelope and is visible in the receipt.
- Append one correlated audit event with actor, source/destination org/workspace/resource, group outcome, grant transition summary, proxy mapping outcome, manifest/key generation, operation/idempotency correlation and result. Never record profile bytes, cookies, key material, credential contents, bearer tokens or plaintext envelopes.

## REST / Tauri / MCP / UI parity

- REST exposes preflight, begin, state/status, connected-device prepare dispatch, commit and cancel through one Rust transfer service. Strict DTOs and OpenAPI document each state and retryable error.
- Tauri orchestrates source stop/sync, OS-vault access, bounded decrypt/re-encrypt, destination upload, resume/cancel, and receipt display.
- MCP invokes the same service and authorization. It may request/inspect/cancel and dispatch preparation to an authorized connected device. It must never accept/return plaintext keys, credentials, or profile contents and cannot commit without the prepared device receipt.
- The desktop flow is a full page (not a modal): source/destination breadcrumbs; destination group/No Group; destination access; proxy mapping/removal; encryption/device summary; estimate; stop requirement; staged progress; retry/cancel; conflict/offline states; committed receipt. No hidden or optimistic local-only success.

## Errors and recovery

Typed states include missing source/destination scope, source forbidden, destination forbidden, profile running/lease active, sync dirty, no eligible connected device, vault key unavailable, source integrity failure, destination quota exceeded, recipient set stale/too large, proxy mapping required/incompatible, staged object mismatch, revision conflict, idempotency conflict, transfer already in progress, and cleanup pending.

Pre-commit errors preserve source metadata/data and do not publish destination access. Post-commit storage cleanup failure does not undo the committed move; it remains visible as cleanup-pending and retriable. Previously copied offline plaintext/keys cannot be erased; the UI must state that limitation.

## Acceptance and ownership

- IO-CONTROL owns dual-scope authorization, resource/grant transaction, transfer journal and audit integration.
- IO-DATA owns encrypted generation conversion, staging, manifest/object validation, recovery and safe cleanup.
- IO-AUTO owns REST/OpenAPI/MCP contract parity and idempotent service invocation.
- IO-CORE owns Tauri/device dispatch, lifecycle stop/sync and OS-vault crypto orchestration.
- IO-UI owns full-page Donut-compliant UX only after the contract stabilizes.
- IO-QA owns adversarial permission matrix, interruption/reopen, sync, no-leak and Windows/macOS evidence.

Acceptance requires that source/destination authorization, exact recipient and proxy rules, all crash points, replay, hashes, old access denial, destination access, and exactly-one-active-binding pass in automated tests and isolated staging. Windows and macOS end-to-end evidence is required before release. No production migration or release promotion is authorized by this design document alone.

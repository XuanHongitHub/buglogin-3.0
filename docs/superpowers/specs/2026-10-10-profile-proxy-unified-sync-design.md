# Profile and Proxy Unified Sync Design

Status: contract/design gate; hosted implementation is blocked until the BugLogin API contract and staging environment are verified. Local Rust contract work may proceed against this draft.

## Product behavior

- A profile owns its proxy selection through the canonical `Profile.proxy_id` reference.
- A proxy does not expose an independent sync toggle. When a profile is opted into hosted sync, its selected proxy configuration is an automatic dependency of that profile's sync set.
- Profiles without hosted sync enabled remain local. No proxy data is sent just because a proxy exists in the local inventory.
- Profile metadata, profile browser data, and referenced proxy state use one BugLogin API authority, one versioned manifest, and one conflict/retry policy.
- A shared proxy referenced by multiple synced profiles has one canonical identity in the workspace. The manifest records references; the proxy payload is uploaded once and reused by content hash.
- A manifest dependency must resolve to immutable, content-addressed proxy metadata and an opaque encrypted credential envelope. A mutable inventory revision alone is not a valid sync dependency because the current proxy repository retains only the latest revision.
- Proxy CRUD accepts an optional `hasCredential` marker in REST and MCP. It records whether the owning client has a local credential, but never accepts a username, password, or secret reference from the API caller. Updates that omit the marker preserve the current credential state.
- Profile DTOs expose the proxy ID and safe proxy metadata only. Credentials are encrypted on the client and stored in the OS vault; transport payloads contain only an opaque encrypted secret envelope/reference. Credentials never enter logs, audit detail, React, or MCP output.

## Sync efficiency

- Use manifest generations, ETags/revisions, idempotency keys, and content hashes to skip unchanged metadata and blobs.
- Send only changed profile metadata, proxy configuration, and browser-data chunks. Do not re-upload an entire browser-data directory when a small set of files changed.
- The v1 profile projection is not incremental: its AES-GCM associated data includes the manifest generation, encryption uses a fresh nonce, and the API's physical blob key includes that generation. Re-encrypting unchanged file bytes therefore creates a new object and a full upload. New v2 payloads use stable workspace/resource/path associated data and publish a key-version fingerprint; v1 remains readable. This establishes safe ciphertext reuse semantics, but caching and server object-reference reuse are still required to reduce transfer. The standalone `sync::chunks` utility uses stable logical chunk associated data and ciphertext-addressed keys, but the profile projection does not use it yet.
- A v2 manifest object may declare `sourceGeneration` to reuse an immutable object. The API accepts it only when the immediately previous active manifest contains the exact key, size, checksum, physical source generation, and same resource-key version. The API resolves the prior physical generation internally; clients cannot provide arbitrary object-store keys. Resource-key rotation invalidates reuse under the previous key until clients re-encrypt.
- Coalesce rapid edits per profile, bound transfer concurrency and chunk size, and resume verified chunks after interruption.
- Profile start/stop and proxy edits must not trigger redundant full scans. Maintain a local change journal or equivalent incremental manifest input, with periodic reconciliation as recovery.
- Enforce per-file and per-profile size limits at the API boundary and return typed quota/validation errors before accepting partial state.
- Use one shared 64 MiB encrypted object limit in the Rust manifest validator, HTTP upload body extractor, and desktop snapshot. Per-profile/workspace aggregate quotas remain enforced separately by workspace usage policy.
- A profile and its proxy dependency become visible atomically at one manifest generation. Missing, unauthorized, stale, or cross-workspace proxy references reject the commit without changing the active profile revision.

## Draft manifest dependency contract

- `SyncManifest.dependencies` is an additive array; older manifests deserialize with an empty array.
- `SyncManifest.payloadEncryption` is an optional versioned declaration for backward-readable manifests. New commits containing payload objects require `schemaVersion: 1`, a supported v1/v2 algorithm, and the canonical resource key-envelope reference at `.buglogin/resource-key-envelope.json` in the same manifest object set. New desktop writes use v2 and include `keyVersion`; v1 remains readable. Each profile/resource has a distinct AES-256-GCM key. Proxy metadata stays safe and explicit as a manifest dependency; the envelope is never projected into browser user data.
- Profile payload objects use the authenticated chunk wire format. The API rejects raw profile blobs before storing them, then checks the declared algorithm, resource-envelope JSON shape/workspace+resource scope, and chunk framing before activation. The server has no key and cannot verify an AES-GCM tag; clients must authenticate every chunk before use.
- Payload encryption v1 binds associated data to workspace, resource, generation, and path. V2 binds to workspace, resource, and path so an unchanged ciphertext can be reused across generations. Its `keyVersion` is the SHA-256 fingerprint of the 256-bit resource key; clients verify it after recovery, and the API rejects an active v2 key-version change until a key-rotation protocol is implemented.
- A proxy dependency contains `kind: "proxy"`, canonical `proxyId`, metadata `revision`, and immutable object references for the safe proxy configuration and optional encrypted credential envelope. Object references bind key, byte length, and SHA-256 checksum and must also appear in the manifest object set.
- The API validates that every dependency belongs to the same workspace, is referenced by the profile's canonical `proxy_id`, is readable by the actor, and has complete verified objects before manifest activation. It validates the proxy-envelope workspace/resource/proxy/revision scope and encrypted wire version both at blob upload and manifest activation.
- A profile with no selected proxy must have no proxy dependency. A profile with a selected proxy must have exactly one matching dependency. Duplicate, stale, missing, unauthorized, and cross-workspace dependencies reject the entire generation.
- Credential envelopes are opaque to the server. A missing, revoked, or wrong-device key must not be treated as a usable proxy credential.
- The Rust `ProxyCredentialEnvelope` wire type uses schema version 1 and AES-256-GCM with the profile resource key. Its authenticated data is `buglogin/proxy-credential/v1/{workspaceId}/{resourceId}/{proxyId}/{proxyRevision}`; opening requires every scope field and revision to match the active manifest dependency. The credential bytes are JSON `{username,password}` and must remain in Rust/OS vault after decryption, never in the browser profile directory, React, operation results, or logs.
- Object bytes remain invisible until the PostgreSQL manifest activation commits. Retries with the same idempotency key and request hash return the original receipt; conflicts preserve the prior active generation.
- The desktop stores each resource data key and recovery key together as one secret in the OS vault. The recovery key wraps only that resource key in the manifest envelope, preserving profile-level ACL boundaries. Tauri exposes an authenticated recovery-code read and recovery operation, but the user-facing recovery flow, code confirmation/backup guidance, key rotation/revocation, and audit evidence still need completion before encrypted sync can ship.

## Authority and security

- Rust owns sync scheduling, proxy/profile serialization, hashing, chunk transfer, retries, conflict decisions, secret encryption/decryption, and API credentials.
- The hosted API owns workspace ACL, canonical profile/proxy IDs, resource bindings, leases/fencing, revisions, quotas, audit, and atomic manifest activation.
- Browser clients use one configured web origin. The API answers credentialed CORS and preflight only for that exact origin and canonical API methods/headers; wildcard origins are forbidden.
- The server must validate the actor's access to both profile and proxy in the same workspace on every read/write; client-side UI state is not authorization.
- Production mode never falls back to Donut endpoints or local files as authority when BugLogin API is unavailable. Existing local/self-host mode remains explicit and isolated.
- Proxy credentials are not silently copied between workspaces or devices. A device lacking the required secret key reports an actionable missing-secret state and cannot launch through that proxy.

## Compatibility and migration

- Existing per-proxy `sync_enabled` values are legacy metadata only. Migration must preserve current data and report proxies that are unreferenced, referenced by unsynced profiles, or referenced across multiple workspaces.
- The migration must not automatically upload every stored proxy. Only dependencies of profiles the user or workspace has explicitly enabled for hosted sync are eligible.
- Remove the proxy sync toggle only after the BugLogin API supports the complete dependency transaction and migration/reconciliation behavior.
- Preserve local proxy use in local mode. A remote API outage must not delete local profiles or proxy credentials.

## Required contract additions before hosted implementation

- Profile/proxy CRUD and profile-to-proxy binding under workspace-scoped authorization.
- Atomic profile-manifest commit that includes proxy dependency revision(s).
- Proxy credential envelope format, device key lifecycle, rotation, revocation, and recovery behavior. The v1 envelope wire type, projection serialization/hydration, REST/MCP metadata marker, API object-scope validation, launch bridge consumption, and local OS-vault input/snapshot wiring exist. Staging verification, shared-device key lifecycle, revocation, and recovery behavior remain unproven.
- Incremental blob transfer protocol with chunk checksums, resume semantics, quota errors, and idempotent finalize.
- ACL, audit, lease/fencing, conflict, idempotency, and error mappings shared by Rust, REST/OpenAPI, and MCP.
- Staging contract tests and Windows/macOS device-vault evidence.

The existing `production-launch-sync-v1.yaml` is a draft dependency and lease contract. It does not currently define immutable proxy objects, proxy-secret envelopes, or atomic profile-plus-proxy activation. Do not wire endpoint behavior or claim this feature production-ready until the contract is versioned and verified against staging.

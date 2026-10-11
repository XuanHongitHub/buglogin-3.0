# Profile and Proxy Unified Sync Design

Status: contract/design gate; hosted implementation is blocked until the BugLogin API contract and staging environment are verified. Local Rust contract work may proceed against this draft.

## Product behavior

- A profile owns its proxy selection through the canonical `Profile.proxy_id` reference.
- A proxy does not expose an independent sync toggle. When a profile is opted into hosted sync, its selected proxy configuration is an automatic dependency of that profile's sync set.
- Profiles without hosted sync enabled remain local. No proxy data is sent just because a proxy exists in the local inventory.
- Profile metadata, profile browser data, and referenced proxy state use one BugLogin API authority, one versioned manifest, and one conflict/retry policy.
- A shared proxy referenced by multiple synced profiles has one canonical identity in the workspace. The manifest records references; the proxy payload is uploaded once and reused by content hash.
- A manifest dependency must resolve to immutable, content-addressed proxy metadata and an opaque encrypted credential envelope. A mutable inventory revision alone is not a valid sync dependency because the current proxy repository retains only the latest revision.
- Profile DTOs expose the proxy ID and safe proxy metadata only. Credentials are encrypted on the client and stored in the OS vault; transport payloads contain only an opaque encrypted secret envelope/reference. Credentials never enter logs, audit detail, React, or MCP output.

## Sync efficiency

- Use manifest generations, ETags/revisions, idempotency keys, and content hashes to skip unchanged metadata and blobs.
- Send only changed profile metadata, proxy configuration, and browser-data chunks. Do not re-upload an entire browser-data directory when a small set of files changed.
- Coalesce rapid edits per profile, bound transfer concurrency and chunk size, and resume verified chunks after interruption.
- Profile start/stop and proxy edits must not trigger redundant full scans. Maintain a local change journal or equivalent incremental manifest input, with periodic reconciliation as recovery.
- Enforce per-file and per-profile size limits at the API boundary and return typed quota/validation errors before accepting partial state.
- A profile and its proxy dependency become visible atomically at one manifest generation. Missing, unauthorized, stale, or cross-workspace proxy references reject the commit without changing the active profile revision.

## Draft manifest dependency contract

- `SyncManifest.dependencies` is an additive array; older manifests deserialize with an empty array.
- A proxy dependency contains `kind: "proxy"`, canonical `proxyId`, metadata `revision`, and immutable object references for the safe proxy configuration and optional encrypted credential envelope. Object references bind key, byte length, and SHA-256 checksum and must also appear in the manifest object set.
- The API validates that every dependency belongs to the same workspace, is referenced by the profile's canonical `proxy_id`, is readable by the actor, and has complete verified objects before manifest activation.
- A profile with no selected proxy must have no proxy dependency. A profile with a selected proxy must have exactly one matching dependency. Duplicate, stale, missing, unauthorized, and cross-workspace dependencies reject the entire generation.
- Credential envelopes are opaque to the server. A missing, revoked, or wrong-device key must not be treated as a usable proxy credential.
- Object bytes remain invisible until the PostgreSQL manifest activation commits. Retries with the same idempotency key and request hash return the original receipt; conflicts preserve the prior active generation.
- This draft does not yet define device key enrollment, recovery, rotation, or revocation wire formats. Those are required before encrypted credential sync can ship.

## Authority and security

- Rust owns sync scheduling, proxy/profile serialization, hashing, chunk transfer, retries, conflict decisions, secret encryption/decryption, and API credentials.
- The hosted API owns workspace ACL, canonical profile/proxy IDs, resource bindings, leases/fencing, revisions, quotas, audit, and atomic manifest activation.
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
- Proxy credential envelope format, device key lifecycle, rotation, revocation, and recovery behavior.
- Incremental blob transfer protocol with chunk checksums, resume semantics, quota errors, and idempotent finalize.
- ACL, audit, lease/fencing, conflict, idempotency, and error mappings shared by Rust, REST/OpenAPI, and MCP.
- Staging contract tests and Windows/macOS device-vault evidence.

The existing `production-launch-sync-v1.yaml` is a draft dependency and lease contract. It does not currently define immutable proxy objects, proxy-secret envelopes, or atomic profile-plus-proxy activation. Do not wire endpoint behavior or claim this feature production-ready until the contract is versioned and verified against staging.

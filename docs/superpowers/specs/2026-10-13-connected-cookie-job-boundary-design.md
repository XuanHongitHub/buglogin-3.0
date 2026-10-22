# Connected Cookie Job Boundary Design

## Problem

The Rust router currently deserializes the storage/service enqueue struct directly. That exposes server-owned actor, idempotency, and timestamp fields as wire inputs even though the handler overwrites only some of them. The router also persists jobs without checking profile ACLs, returns HTTP 200 despite an accepted-job receipt, and the REST paths have no canonical OpenAPI contract.

## Decision

Keep HTTP input separate from the durable job model. The caller may provide command/workspace/device/profile/generation and only the optional source or opaque staged reference required by the command. Derive actor from authenticated session, idempotency from the header, and creation/expiry from the API clock with the existing ten-minute contract window. Reject unknown fields.

Authorize the destination against its profile binding and command capability before persisting. For `cookie.copy`, independently authorize source profile read access and require its bound workspace to match the supplied source workspace. Cookie contents, filesystem paths, vault data, and device-side execution proof never enter this API.

Document the actual queue endpoints as a control-plane contract. Enqueue returns HTTP 202; device inbox/claim/ack/complete require device bearer auth. Inbox, claim, and acknowledgment are bound to the registered device and current executor ACL, so revoked access cannot start execution after a job has been claimed. This describes queue tracking only and does not claim that desktop polling, local cookie mutation, or production sync is complete.

Completion is accepted only after the profile's current hosted manifest proves that encrypted browser data reached at least the receipt's `newGeneration`. Resolve the job's server-owned profile binding and inspect the current manifest under the refreshed control snapshot and mutation lock. Reject a receipt while the manifest is absent, behind, or has no encrypted non-internal profile files; return a retryable conflict so the device can resubmit after sync commits. Replaying the exact receipt for an already-completed job remains idempotent without depending on the profile's current binding or manifest.

## Acceptance

- Wire enqueue schema excludes actor, idempotency, created time, expiry, and unknown fields.
- Enqueue rejects missing/incorrect workspace bindings, archived destinations, and missing destination/source ACL before any durable write.
- Creation time and ten-minute expiry are server-generated.
- HTTP returns 202 and stored idempotent replay returns the same job identity.
- OpenAPI has every cookie-job route, query, body, response and status; generated DTOs include those schemas.
- Device inbox lists only jobs bound to that device and currently permitted to its account; claim and acknowledgment recheck destination create and copy-source read ACL.
- Job lookup does not reveal whether another device owns a guessed job ID.
- A running job cannot become completed until its bound profile has an active encrypted manifest at or beyond the receipt generation; exact completed-receipt replay remains idempotent after later profile lifecycle changes.
- Targeted API tests and contract/web generation gates pass.

## Boundary

No cookie values are sent to the API. Native CookieAuthority mutation, desktop inbox polling/retry, stage handshake, runtime stop/lease proof, and Windows/macOS execution evidence remain separate release blockers.

## Native Wayfern ciphertext contract

- BugLogin must never persist imported cookie values in SQLite's plaintext `value` column. The Donut writer is reference-only because it intentionally does that.
- The Wayfern profile's `os_crypt_key` is the encryption input; read it as raw bytes without trimming, derive the runtime AES-128 key using PBKDF2-HMAC-SHA1 (`saltysalt`, 1 iteration on Windows/Linux and 1003 on macOS), prepend `SHA-256(host_key)` to the cookie value, and write Chromium `v10` AES-CBC ciphertext only to `encrypted_value`.
- Zeroize the raw profile key, derived AES key, and temporary plaintext buffer on scope exit. Never include them or cookie values in errors, logs, or API receipts.
- Missing/empty key, unsupported cookie schema, running profiles, or missing database must fail closed. Do not create a guessed Cookie schema for profiles that have not initialized the browser database.
- The ciphertext primitive and profile-database writer are separate from job dispatch. A dispatcher may acknowledge success only after a crash-recoverable transaction reconciles the Cookie DB with CookieAuthority generation/idempotency state; a local vault update alone is not a browser cookie mutation.
- Device-local stage values must survive process restart only until a short fixed expiry (15 minutes) in the OS vault. A per-profile forward-recovery journal binds the hosted job/operation/device/profile, stage, idempotency key, and generation transition; while pending, it fences launch and further cookie mutations. Recovery is redo-only across the authority ledger and Wayfern SQLite transaction, then removes the journal and permits a terminal device receipt. Cookie values and vault references never leave the device.
- Hosted recovery identity is persisted atomically with the local mutation receipt. The journal carries only job ID, operation ID, and authenticated device ID alongside existing scope, generation, stage, idempotency, and fencing data. Restore rejects malformed identity; it never infers the job from timestamps or client-supplied profile names.

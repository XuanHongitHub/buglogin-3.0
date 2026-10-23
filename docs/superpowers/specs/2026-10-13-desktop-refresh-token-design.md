# Desktop Refresh Token Design

Status: local refresh implementation exists. Startup now refuses to restore a cached access token from a vault pair marked refreshPending; hosted restart, native-vault lifecycle, and platform evidence remain required before production auth acceptance.

## Problem

The desktop exchanges a one-time device authorization for a one-hour bearer and stores only that bearer in the OS vault. The existing `POST /v1/auth/refresh` rotates an access bearer only while it is still valid. A desktop that remains open past expiry or is closed for more than one hour cannot restore its session.

## Contract

- Device authorization issues a short-lived access token and a separate opaque refresh token.
- Refresh tokens are random, device-bound, family-bound, short enough for OS-vault storage, and persisted only as SHA-256 hashes. Access remains one hour; refresh lifetime is 30 days and is not extended beyond the family’s absolute expiry.
- `POST /v1/auth/device/refresh` accepts the refresh token in a strict JSON body over HTTPS and returns a new access/refresh pair. It does not accept an access token as refresh authority.
- Refresh rotates credentials atomically. When the caller supplies an optional 16-128 ASCII character `Idempotency-Key`, the consumed token stores an encrypted copy of the exact response for a five-minute retry window; a same-key retry replays it, including after a lost response or a concurrent CAS loss. A different key or a retry without a key is treated as reuse and revokes the family. The replay copy is AES-GCM encrypted with a per-token key derived by HKDF-SHA256 from the presented old token, and is bound with AAD to the flow, token hash, key hash, and device ID. Logout, credential-version changes, account recovery, and owner session revocation invalidate refresh credentials through the same family/session authority.
- Replay-window expiry and persisted session/refresh revisions use checked arithmetic. Overflow fails before writing or returning a refresh result; it must not saturate or wrap into a valid-looking deadline/revision.
- Newly issued device refresh-token buffers stay in zeroizing owners until the atomic persistence operation succeeds and ownership is moved to the response object.
- Account status is read from the durable account record before device-session issuance or refresh rotation, and its record revision is included as a read-only CAS precondition in the same durable transaction that creates or rotates the session/refresh pair. This closes the cross-API-instance race that a process-local auth lock alone cannot close. Inactive accounts cannot receive a new pair; a concurrent status transition makes the CAS fail without creating or consuming session credentials. Refresh CAS conflicts are retryable, so the desktop preserves the OS-vault pair and retries with the same idempotency key.
- Refresh-token lookup is hash-indexed; it must not scan all sessions. Raw tokens are never stored, logged, audited, or returned after issuance.
- A persisted access-session/refresh-token pair must agree on account, device, family, and credential version before it can rotate. A mismatched pair is corrupt and must fail closed; it must never mint a replacement pair with inconsistent password-reset revocation state.
- Every device refresh record must resolve to an existing same-account family record. The legacy access-session compatibility path may tolerate pre-family sessions, but that exception does not apply to device refresh credentials.
- Account-scoped device/family/session enumeration must use a storage query constrained by account ID, backed by a PostgreSQL JSONB expression index; it must not materialize every account's auth records to answer one account's request.
- Desktop stores the pair in the native OS vault with versioned replacement. Startup refreshes an expired/near-expiry access token before declaring the session authenticated. Successful in-process refresh updates the token used by API and coordination requests atomically. Logout removes both local credentials after server revocation succeeds. In-memory API response, vault, API-client, and coordination bearer owners zeroize retained token buffers when dropped.
- Legacy vault entries containing only an access bearer remain readable; if they cannot refresh, the app clears the stale entry and requires device authorization again.

## Failure behavior

- Restart recovery invariant: when the persisted pair has refreshPending=true, bootstrap must not load its cached access token into the API client or report the session authenticated. Preserve the vault pair for diagnostics/replacement, disable local authority, and require fresh device authorization without replaying the one-time refresh token.

- Before sending a one-time refresh credential, the desktop atomically marks the OS-vault pair `refreshPending`. During the same process attempt, bounded retries may resend the old credential only with the identical deterministic `Idempotency-Key`; the API can then replay the encrypted response if rotation already committed. On success the desktop replaces the pair and clears the marker.
- If the process restarts while `refreshPending` is set, it preserves but disables that pair locally and requires fresh device authorization; it does not replay the old credential across startup. This keeps crash recovery fail-closed while allowing safe same-process recovery from a lost response.
- Network/transient failures preserve the vault pair and report an offline/retryable state. A definitive non-retryable auth response removes it. An ambiguous result after bounded same-key retries leaves it staged for replacement by a fresh authorization.
- Invalid, expired, revoked, replayed, or device-mismatched refresh credentials clear local auth only after the server gives a definitive non-retryable auth response.
- Concurrent refresh requests are serialized on desktop and CAS-protected on the API. An ambiguous network result may retry only with the same idempotency key and within the bounded in-process retry policy; the pending marker survives restart and the client re-authenticates if the replacement pair cannot be recovered.

## Compatibility

- Keep `POST /v1/auth/refresh` bearer rotation for existing non-desktop clients during migration.
- Add a distinct device-refresh route and response contract; device authorization responses gain a required refresh token.
- Update canonical OpenAPI, command registry, generated client artifacts, and API compatibility tests together.

## Acceptance

- API tests prove hash-only credential persistence, encrypted replay persistence, 30-day absolute expiry, atomic rotation, same-key response replay after lost responses/concurrent CAS, different-key and no-key family revocation, device binding, logout/recovery/session-revoke invalidation, and no raw-token audit/log leakage.
- Desktop tests prove OS-vault pair round-trip/versioned rotation, startup restore after access expiry, legacy bearer fallback, concurrent refresh serialization, in-process coordination token update, transient failure preservation, and logout cleanup.
- REST/OpenAPI/generated contracts agree. Hosted PostgreSQL restart/replay tests and Windows/macOS OS-vault evidence remain release gates.

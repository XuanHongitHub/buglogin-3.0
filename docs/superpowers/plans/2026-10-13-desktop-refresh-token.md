# Desktop Refresh Token Implementation Plan

Status: implementation in progress; local API and desktop code is implemented and targeted gates pass. Native-vault lifecycle, hosted restart, and platform evidence remain open.

Spec: `docs/superpowers/specs/2026-10-13-desktop-refresh-token-design.md`.

## API and identity

- [x] Add a hash-indexed refresh credential record with account, family, device, absolute expiry, and consumed state; never persist raw refresh tokens.
- [x] Add atomic issue and rotate persistence commands that update access sessions and refresh indexes together.
- [x] Add device-session issue/refresh service methods; serialize concurrent refresh, replay the encrypted exact response for same-key retries within five minutes, and revoke the family on different-key or no-key reuse.
- [x] Ensure logout, credential-version changes, recovery, and owner session revocation invalidate the refresh family.
- [x] Add strict `POST /v1/auth/device/refresh` DTOs and return the access/refresh pair from device exchange.
- [x] Check account activity and fence device-session issuance and refresh rotation against cross-instance status changes by including the durable account-record revision in the same control-store CAS transaction. Existing suspended-account HTTP regression covers denial; a new storage regression covers the status-change race for both issuance and rotation and verifies no token records change. Refresh CAS conflicts are retryable so desktop credentials survive benign concurrent changes; execution is deferred to the consolidated pass.
- [x] Apply the same durable account-revision CAS to legacy bearer-session refresh rotation; the HTTP path now reads an active versioned account and the storage transaction fences account and session writes together. Regression source is added but not run.
- [x] Reject refresh replay-deadline and persisted session/refresh revision overflow before the atomic rotation; source updated, regression and build deferred to the consolidated verification pass.
- [x] Keep candidate device refresh-token buffers zeroizing until persistence succeeds; source updated, consolidated verification deferred.
- [x] Update canonical OpenAPI, command registry, generated contracts, and compatibility coverage.
- [x] Bound account-scoped active-device, family-revocation, and session-revocation reads at storage, with a PostgreSQL JSONB account index instead of scanning every account's records. A memory-store regression source covers tenant/type/account isolation; execution is deferred to the consolidated verification pass.
- [x] Route authenticated session listing through the same account-scoped PostgreSQL query and validate each returned record ID/account binding before exposing summaries. Existing persistent session-list and live storage account-scope regression sources cover the behavior; execution remains deferred to the consolidated verification pass.
- [x] Validate that device access-session and refresh records agree on credential version before rotation; mismatch fails closed. Regression source is written and deferred to the consolidated verification pass.
- [x] Require every persisted device refresh record to resolve to its canonical lowercase family ID and same-account family record; missing family records fail closed. Regression source is written and deferred to the consolidated verification pass.

## Desktop

- [x] Refuse a cached access token at bootstrap when the durable vault pair has refreshPending=true; expose unauthenticated state and require fresh device authorization without replaying the refresh token. The persisted-session access-token selector returns no token while the marker is set, and the vault regression covers both normal restoration and pending refusal.

- [x] Store access/refresh pair and expiry together in the OS vault with versioned CAS replacement; read legacy bearer entries and durably mark one-time refresh attempts before sending.
- [x] Refresh on startup when access token is expired/near expiry and preserve credentials on transient failures.
- [x] Serialize refresh and atomically update the bearer used by REST and coordination clients.
- [x] Clear stale local auth only on definitive refresh rejection; remove pair on logout after server revocation.
- [x] Preserve the legacy bearer session after a retryable refresh failure by clearing its local pending marker and returning the retryable problem. Device and legacy refresh now share the same error classification; a regression test covers transient, permanent, and revoked-session results. Source-only, consolidated verification deferred.
- [x] Zeroize access/refresh bearer copies from API response DTOs and OS-vault wrapper values on drop; keep the API client's current/refreshed bearer slots, desktop's retained refresh token, and shared coordination bearer in zeroizing owners. Source-only; consolidated verification deferred.
- [x] Store current/refreshed API bearer slots under one lock so refresh replacement and logout clearing are atomic; setting a new current token clears any stale refreshed bearer. Source-only; consolidated verification deferred.
- [x] Preserve desktop auth credentials when device refresh returns a transport failure, truncated body, or server 5xx after the same-key retry. These outcomes remain retryable because the API may have committed rotation before the response was lost; add transport regression coverage and defer execution to the consolidated verification pass.

## Verification and rollout

- [ ] Add API tests for hash-only storage, absolute expiry, hosted restart, concurrent CAS rotation, replay, device binding, revoke paths, and redaction. Local rotation/replay/revoke/binding/expiry tests pass; hosted restart evidence is open.
- [ ] Add desktop tests for real OS-vault migration/restore after downtime, refresh races, pending-marker restart recovery, coordination rebinding, offline handling, and logout. The pending-marker token selector is unit-tested; native-vault and full state lifecycle coverage remain open.
- [x] Run targeted Rust/API/desktop and contract gates (desktop lib 89/89, API auth 10/10, HTTP auth 10/10, command registry 7/7, contracts 24/24; formatting and diff checks pass).
- [ ] Run hosted PostgreSQL restart/replay rehearsal and Windows/macOS OS-vault evidence before production acceptance.

No production route, auth policy, or deployed configuration changes are authorized by this plan alone.

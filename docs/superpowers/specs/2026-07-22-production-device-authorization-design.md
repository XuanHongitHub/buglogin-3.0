# Production Device Authorization Design

Status: implementation wave, 2026-07-22. Refresh retry semantics were superseded by the [2026-10-13 desktop refresh-token design](2026-10-13-desktop-refresh-token-design.md).

## Scope

Add the production desktop authorization path without exposing account passwords to the desktop app. Existing bearer refresh/logout remain compatible.

## Protocol

- `deviceId` has one shared syntax across authorization, refresh, active sessions, device-key registration, encrypted key envelopes, and desktop OS-vault keying: 8–128 ASCII bytes, first character alphanumeric, remaining characters alphanumeric or `.`, `_`, `-`. The desktop's generated `dev_<32 lowercase hex>` identifier conforms. Runtime-projection `deviceId` is a separate local runtime identity and does not use this auth identity syntax.

- `POST /v1/auth/device/start` is unauthenticated and accepts `deviceId` plus a display-only `clientName`.
- Response returns one-time `deviceCode`, human `userCode`, verification URI, `expiresIn`, and polling `interval`.
- Packaged desktop accepts only the exact HTTPS API host `api3.buglogin.com` while `api.buglogin.com` still serves 1.0. Device verification resolves to `app3.buglogin.com/device`, without port, query, user information, or fragment. Local development may use loopback HTTP. Re-enable the `api.buglogin.com` API host only after its verified DNS/API cutover to 3.0.
- `POST /v1/auth/device/approve` requires an active account bearer and accepts `userCode`.
- `POST /v1/auth/device/token` accepts `deviceCode` and returns `authorization_pending`, `slow_down`, `access_denied`, or `expired_token`, or a bearer session after approval.
- Device codes and refresh tokens are opaque, case-sensitive values in the exact 64-character lowercase hexadecimal form issued by BugLogin; clients must not uppercase or otherwise normalize them.
- Polling starts at five seconds. Early polling returns `slow_down` and increases the durable interval by five seconds.

## Persistence and security

- Device and user codes use independent cryptographic random values.
- Only SHA-256 hashes are persisted. Raw codes are returned once and never logged, audited, stored, or exposed to MCP.
- Desktop/API transient DTOs zeroize raw device codes, user codes, access tokens, and refresh tokens on drop. Debug formatting for device-authorization values must redact both codes.
- Token-bearing transient DTOs do not implement `Clone` unless a concrete ownership flow requires it; transfer owned code strings into pending state instead of copying them.
- Approval binds the authorization to the authenticated account and requested `deviceId`.
- Exchange rechecks the approved password credential version against the current account credential state before issuing a session; password reset invalidates pending device approvals.
- Token exchange consumes the authorization record and its verified user-code index using their exact stored revisions in the same control-store transaction that persists the access session, refresh session, family, and account lifecycle revision. A failed transaction leaves both the authorization and session state unchanged; CAS makes concurrent exchanges single-use. The auth layer accepts only an opaque, service-created consumption value rather than arbitrary record deletions.
- Device sessions preserve `deviceId` through refresh rotation.
- Device refresh rotation supports a bounded same-key `Idempotency-Key` replay as specified in the linked 2026-10-13 design. A different key or retry without a key revokes the family. The replacement refresh token keeps the original absolute expiry.
- Expired, denied, consumed, unknown, malformed, and cross-account approvals fail closed.
- Local/self-host and hosted use the same Rust authority; production desktop still talks only to hosted HTTPS API.

## Non-scope

Browser password login, recovery, web cookie/CSRF transport, API tokens, and organization SSO remain separate identity waves.

## Acceptance

- Codes and sessions survive API restart.
- Poll interval and `slow_down` survive restart.
- Approval and exchange are single-use and race-safe through CAS revisions.
- Raw device/user/session tokens never appear in persisted JSON or audit metadata.
- Device-bound refresh preserves the same `deviceId`.
- REST/OpenAPI and desktop remote client tests pass.

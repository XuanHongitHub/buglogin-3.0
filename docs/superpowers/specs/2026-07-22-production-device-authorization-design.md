# Production Device Authorization Design

Status: implementation wave, 2026-07-22.

## Scope

Add the production desktop authorization path without exposing account passwords to the desktop app. Existing bearer refresh/logout remain compatible.

## Protocol

- `POST /v1/auth/device/start` is unauthenticated and accepts `deviceId` plus a display-only `clientName`.
- Response returns one-time `deviceCode`, human `userCode`, verification URI, `expiresIn`, and polling `interval`.
- `POST /v1/auth/device/approve` requires an active account bearer and accepts `userCode`.
- `POST /v1/auth/device/token` accepts `deviceCode` and returns `authorization_pending`, `slow_down`, `access_denied`, or `expired_token`, or a bearer session after approval.
- Polling starts at five seconds. Early polling returns `slow_down` and increases the durable interval by five seconds.

## Persistence and security

- Device and user codes use independent cryptographic random values.
- Only SHA-256 hashes are persisted. Raw codes are returned once and never logged, audited, stored, or exposed to MCP.
- Approval binds the authorization to the authenticated account and requested `deviceId`.
- Token exchange consumes the authorization before session issue. If session persistence fails after consume, the code stays terminal and the desktop must start a new flow; duplicate sessions are never issued.
- Device sessions preserve `deviceId` through refresh rotation.
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

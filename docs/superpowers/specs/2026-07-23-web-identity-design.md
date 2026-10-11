# BugLogin Web Identity Design

Status: implemented backend/web flows; hosted delivery and platform evidence pending.

## Authority

Rust owns credential verification, canonical identity, throttling, sessions, CSRF issuance/validation, recovery tokens, device approvals, revocation, audit, and API authorization. React only renders safe projections and submits forms over same-origin HTTPS.

## Records

- `account_identity`: account ID, canonical username, canonical email, verification state, revision, timestamps.
- `account_password`: account ID, Argon2id PHC hash, password revision, changed-at, failed-attempt counter, lock-until, timestamps.
- `web_session`: opaque random session ID/hash, account ID, family ID, created/last-seen/expires-at, revoked-at, user-agent summary, IP prefix, revision.
- `csrf_secret`: session-bound secret hash; raw token only emitted in a secure cookie/header flow.
- `recovery_token`: random one-time hash, account ID, expiry, consumed-at, revision.

Canonical uniqueness is enforced on trimmed Unicode-lowercased username/email values and never changes the user-facing display value. Full Unicode normalization remains a versioned migration decision before non-ASCII identifiers are enabled.

## Password policy

Argon2id only. Parameters live in the versioned security config. Login verifies the stored PHC string, upgrades parameters after successful verification, increments failure state atomically, and applies bounded account/IP throttling. Lockout returns a generic response and never reveals account existence.

## Browser transport

- Login/logout/refresh use same-origin HTTPS endpoints.
- Session cookie: `HttpOnly; Secure; SameSite=Lax; Path=/`; no bearer in JavaScript, URL, logs, audit, MCP, or screenshots.
- State-changing requests require a session-bound CSRF token in a custom header. CSRF token is never accepted from URL parameters.
- API requests authenticate from the cookie; desktop device flow remains separate.

## Required endpoints

`POST /v1/auth/login`, `POST /v1/auth/logout`, `POST /v1/auth/refresh`, `GET /v1/auth/session`, `GET /v1/auth/sessions`, `DELETE /v1/auth/sessions/{id}`, `POST /v1/auth/recovery/request`, `POST /v1/auth/recovery/complete`, `GET /v1/auth/device/approve`, `POST /v1/auth/device/approve`.

Error responses are generic for login/recovery enumeration resistance, include correlation IDs, and never return password or token material.

## Account lifecycle

New accounts require verified identity before privileged access. Password change and recovery revoke the current session family by default. Admin suspension/disable revokes all sessions. Session/device lists expose safe metadata only.

## Gates

Backend tests cover normalization, uniqueness races, Argon2 verification/upgrade, throttling/lockout, cookie flags, CSRF replay/mismatch, rotation, revoke persistence, recovery single-use, device approval, redaction, and restart reload. Web tests cover login, error states, expiry, retry, logout, and no-secret DOM exposure.


## Implemented recovery and API-token delta — 2026-07-23

- Recovery request persists only a SHA-256 token hash, applies a durable identifier rate window, returns an enumeration-safe response, and delivers the raw token synchronously through the configured trusted HTTPS webhook.
- Recovery completion is idempotency-key bound, consumes the token atomically with the Argon2id password update, then revokes all existing account sessions.
- API tokens use a `blapi_` opaque raw value returned once; only its SHA-256 hash and safe metadata persist. Tokens have bounded expiry, owner-scoped list/revoke, audit, and capability/action scopes enforced as an additional deny boundary by the Rust permission model.
- Recovery and API-token commands remain REST-only. MCP receives no raw token command and can authenticate only with a pre-created scoped token.
- Production startup fails closed without `BUGLOGIN_RECOVERY_WEBHOOK_URL` and `BUGLOGIN_RECOVERY_WEBHOOK_SECRET`; the URL must use HTTPS.

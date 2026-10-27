# BugLogin Web Identity Design

Status: implemented backend/web flows; hosted delivery and platform evidence pending.

## Authority

Rust owns credential verification, canonical identity, throttling, sessions, CSRF issuance/validation, recovery tokens, device approvals, revocation, audit, and API authorization. React only renders safe projections and submits forms over same-origin HTTPS.

## Records

- `account_identity`: account ID, canonical username, canonical email, verification state, revision, timestamps.
- `account_password`: account ID, Argon2id PHC hash, password revision, changed-at, failed-attempt counter, lock-until, timestamps.
- `web_session`: opaque random session ID/hash, account ID, family ID, created/last-seen/expires-at, revoked-at, user-agent summary, IP prefix, revision.
- `session_family`: account ID, family revocation time, and maximum refresh expiry; keep expired session/refresh hashes for 30 days after expiry to support revocation and refresh-reuse detection, then remove the family and its child records atomically.
- `csrf_secret`: session-bound secret hash; raw token only emitted in a secure cookie/header flow.
- `recovery_token`: random one-time hash, account ID, expiry, consumed-at, revision.

Canonical uniqueness is enforced on trimmed Unicode-lowercased username/email values and never changes the user-facing display value. Full Unicode normalization remains a versioned migration decision before non-ASCII identifiers are enabled.

## Password policy

Argon2id is the only steady-state password format. Parameters live in the versioned security config. One-time migrated 1.0 accounts may retain a Node-compatible scrypt verifier inside the encrypted migration package and the account-password record until first successful login. Login verifies that legacy verifier under the same bounded admission and lockout controls, atomically replaces it with Argon2id, and preserves the password version. Password reset always removes any legacy verifier. Lockout returns a generic response and never reveals account existence.

If a credential CAS conflict occurs after password verification, a login may continue only after verifying the submitted password against the latest record. The session must receive that latest record's password version, so a concurrent reset cannot mint a stale-version session.

## Browser transport

- Login/logout/refresh use same-origin HTTPS endpoints.
- Session cookie: `HttpOnly; Secure; SameSite=Lax; Path=/`; no bearer in JavaScript, URL, logs, audit, MCP, or screenshots.
- State-changing requests require a session-bound CSRF token in a custom header. CSRF token is never accepted from URL parameters.
- API requests authenticate from the cookie; desktop device flow remains separate.

## Required endpoints

`POST /v1/auth/login`, `POST /v1/auth/logout`, `POST /v1/auth/refresh`, `GET /v1/auth/session`, `GET /v1/auth/sessions`, `DELETE /v1/auth/sessions/{id}`, `POST /v1/auth/recovery/request`, `POST /v1/auth/recovery/complete`, `GET /v1/auth/device/approve`, `POST /v1/auth/device/approve`.

Error responses are generic for login/recovery enumeration resistance, include correlation IDs, and never return password or token material.

Identity writes validate display-to-canonical username/email consistency and a nonzero revision before opening the multi-record persistence transaction. A malformed identity must not leave an identity row or partial unique-index rows behind.

Recovery throttling uses the account ID for known identities so email and username aliases share one account quota. Unknown identities use only a hash of the normalized submitted identifier and retain the same response shape.

## Account lifecycle

New accounts require verified identity before any web or desktop credential session can be issued. `Pending` identities are rejected by the shared credential authentication service with the same `invalid_credentials` response as unknown identities, after password verification; this prevents identity enumeration and applies to both `/v1/auth/login` and `/v1/auth/desktop/login`. A pending identity must not produce a session, update a successful-login credential record, or upgrade a legacy password verifier. Migrated identities already marked `Verified` remain eligible to authenticate.

The one-time hosted first-owner bootstrap is an explicit operator-attestation exception, not email delivery or automated proof. Before invoking the production-only CLI, the operator must verify the intended owner's control of the submitted email through an approved out-of-band process. The CLI requires the operator to retype the exact email after displaying that requirement; the bootstrap API refuses calls without the attestation flag. In the same atomic transaction that creates the initial identity, password, account, workspace, platform-admin membership, and bootstrap record, the identity is marked `Verified`; the audit detail records `operator-attested-out-of-band`. This process relies on a trusted production operator and the separately controlled out-of-band verification procedure. It does not claim the application itself verified the mailbox. Ordinary account provisioning continues to create `Pending` identities until a supported verification flow is completed. Password change and recovery persistently revoke the account's session families and API tokens. Admin suspension/disable atomically revokes all persisted session families and API tokens with the account status transition, so reactivation cannot revive old browser, desktop, or API credentials. Session/device lists expose safe metadata only.

Web login must check the current account lifecycle record after verifying the password and before issuing cookies. Durable browser-session, device-session/refresh, and API-token issuance include the account record's current revision in the same transaction and advance that revision. This shared write fence serializes credential issuance with status transitions, including the race where a suspension has already enumerated credentials for revocation. A conflict causes web login to restart and API-token creation to return a retryable account-state conflict. Every subsequent browser-cookie session read or mutation and device-token exchange must also check the current account record; otherwise an already-issued cookie or approved device code could continue through web-only paths after suspension even though bearer-authenticated routes reject the account. Suspended or disabled accounts receive `account_inactive` (403), and missing account records fail as invalid identity state. Logout remains available to clear an inactive account's cookie.

An approved device authorization is single-use. Consumption deletes its durable authorization record and user-code index atomically, with the record revision as a compare-and-swap condition. PostgreSQL control-store deletes with an expected revision must verify that exactly one row was deleted; a concurrent stale delete returns `storage_revision_conflict`. This prevents successful device exchanges from accumulating consumed records while preserving one-winner exchange semantics.

## Gates

Backend tests cover normalization, uniqueness races, Argon2 verification/upgrade, throttling/lockout, cookie flags, CSRF replay/mismatch, rotation, revoke persistence, recovery single-use, device approval, redaction, and restart reload. Web tests cover login, error states, expiry, retry, logout, and no-secret DOM exposure.


## Implemented recovery and API-token delta — 2026-07-23

- Recovery delivery is disabled for this release. Recovery request and completion fail closed before creating or consuming credentials; the retained delivery trait exists only for isolated service-level tests and is not wired from production configuration.
- Recovery completion is idempotency-key bound and atomically consumes the token, updates the Argon2id password, revokes existing sessions, and revokes every active API token in one control-store transaction.
- When delivery is enabled for isolated service use, completing one recovery token also consumes every other outstanding token for that account in the same transaction, so an older delivered link cannot reset the password afterward.
  - API tokens use a `blapi_` opaque raw value returned once; only its SHA-256 hash and safe metadata persist. The one-time response DTO zeroizes its raw token after serialization. Tokens have bounded expiry, owner-scoped list/revoke, audit, and capability/action scopes enforced as an additional deny boundary for regular principals by the Rust permission model. An authenticated PlatformAdmin account role overrides token scopes; explicit DENY grants and account status still apply.
- One-time API, recovery, and device-authorization secret candidates remain in zeroizing owners until durable persistence and controlled response/delivery transfer. Temporary random byte buffers are zeroized on success and entropy failure; device-code `Debug` output is redacted. HTTP response DTOs zeroize one-time secrets after serialization.
- Recovery and API-token commands remain REST-only. MCP receives no raw token command and can authenticate only with a pre-created scoped token.
- Outbound recovery delivery is currently unsupported by product decision. Production startup does not require webhook/mail credentials; recovery request and completion fail closed with an unavailable response and must not create, consume, or mutate recovery credentials. If delivery is enabled in a future release, it requires the trusted HTTPS webhook configuration and secret validation described above.

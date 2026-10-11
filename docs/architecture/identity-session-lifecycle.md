# Identity Session Lifecycle

Status: partial implementation contract, 2026-07-22.

## Implemented boundary

- Session tokens are 256-bit random values issued by Rust.
- Only SHA-256 token hashes are persisted; raw bearer values are returned once and never stored, audited, logged, or exposed to MCP.
- Session records persist account, hash, expiration, revocation timestamp, family ID, optional device/CSRF binding, credential version, and optimistic revision in the control record store. Older records without the optional fields remain readable.
- Persistent authentication resolves the SHA-256 token and family state against the shared control store on each request, so API instances observe sessions issued, rotated, or revoked by other instances without caching the full session table at startup.
- Revocation persists before in-memory state changes; storage failure fails closed.
- Bearer `POST /v1/auth/refresh` rotates old/new session records in one control-store transaction; `POST /v1/auth/logout` persists revocation.
- Refresh preserves an existing session family ID, CSRF binding, and creation time on the replacement record; it advances `lastSeenAt` while the prior token is revoked atomically.
- New sessions receive a random family ID stored in an independent control record. Reuse of a revoked refresh token atomically revokes that family, and authentication checks the family record so already-issued replacements stop working across API instances without rewriting every session. Concurrent refresh conflicts re-read the source token and trigger the same family revocation if another request rotated it first.
- Password recovery updates the credential, consumes the recovery token, and revokes all active sessions in one control-store transaction before the response succeeds.
- Password-authenticated browser and device sessions carry the password version used during authentication. Rust checks that version against the shared credential record on every request, so an in-flight login or device approval cannot create a usable old-credential session after recovery commits.
- Expired, revoked, unknown, and hash-mismatched tokens remain rejected by Rust authority.

## Required next contract

- Account credential and verified-identity schema.
- Browser HttpOnly Secure SameSite cookie transport and CSRF policy.
- Cross-origin browser transport uses an exact configured web origin, credentialed CORS, and preflight support for canonical API headers. Wildcard origins are forbidden with credentials.
- Desktop device-code authorization carries the approved credential version into the issued device session. Device-level list/revoke and web screenshot QA remain future work.
- Login credential flow, browser HttpOnly cookie/CSRF transport, session list/revoke, device list/revoke, recovery, and scoped API tokens are implemented in Rust and REST with targeted HTTP tests. Recovery tokens and API tokens persist only SHA-256 hashes; password reset consumes the recovery token atomically with the credential update and revokes existing sessions. Production recovery delivery uses a required HTTPS webhook with a Rust-only bearer secret. API token capability/action scopes are an additional deny boundary inside the canonical domain ACL. Account Security and recovery UI are implemented; browser screenshot QA and hosted provider evidence remain pending. Bearer logout/refresh are implemented.
- Rate limits, abuse lockout, recovery audit, key rotation, and web E2E.

No web credential may be placed in `NEXT_PUBLIC_*`, URL parameters, browser logs, screenshots, MCP output, or telemetry.

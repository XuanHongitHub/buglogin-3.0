# Web Identity Implementation Plan

1. Add domain identity records and normalization.
2. Add durable credential/session/recovery persistence.
3. Add Rust login, CSRF, recovery, device approval.
4. Publish OpenAPI and command-contract deltas.
5. Add web auth gate and security UI.
6. Run backend, contract, browser, restart tests.
7. Re-audit migration and release blockers.

## Ownership

- Domain: `crates/domain/src/account/`, `crates/domain/src/identity/`
- API: `crates/api/src/auth/`, `crates/api/src/services/`, `crates/api/src/http/`
- Contracts: `docs/contracts/`, `crates/api/openapi/`
- Web: `frontend/src/apps/web/`
- Tests: adjacent crate/frontend test locations only

Production migration remains blocked until rehearsal, rollback, trusted adapter, and Windows/macOS evidence pass.

## Progress — 2026-07-23

- [x] Durable recovery token/rate records and Argon2id reset.
- [x] Trusted HTTPS webhook delivery and production fail-closed configuration.
- [x] Scoped API token issue/list/revoke and domain ACL scope enforcement.
- [x] REST/OpenAPI/generated command contracts and web account/recovery UI.
- [x] Rust/domain/API/frontend/contract targeted gates.
- [x] Add a four-slot per-process Argon2id verification admission limit with retryable 503 overload; preserve persistent five-attempt account lockout and generic unknown-identity behavior. Unit/auth/contract tests pass. This does not replace distributed gateway rate limiting.
- [ ] Hosted PostgreSQL recovery/API-token restart E2E.
- [ ] Hosted webhook sink evidence and browser screenshot QA.
- [ ] Windows/macOS release evidence and migration rehearsals.

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

Production migration remains blocked until the controlled one-pass clone has its rollback checkpoint, trusted adapters, and Windows/macOS evidence. The owner has waived repeated rehearsal cycles.

## Progress — 2026-07-23

- [x] Durable recovery token/rate records and Argon2id reset.
- [x] Keep recovery delivery explicitly unsupported; production endpoints fail closed without creating recovery credentials or tokens.
- [x] Scoped API token issue/list/revoke and domain ACL scope enforcement.
- [x] REST/OpenAPI/generated command contracts and web account/recovery UI.
- [x] Rust/domain/API/frontend/contract targeted gates.
- [x] Add a four-slot per-process Argon2id verification admission limit with retryable 503 overload; preserve persistent five-attempt account lockout and generic unknown-identity behavior. Unit/auth/contract tests pass. This does not replace distributed gateway rate limiting.
- [x] Recheck account lifecycle after password verification and before issuing browser cookies; add an inactive-account HTTP-handler regression source. Execution is deferred to the user's consolidated verification pass.
- [x] Serialize browser/device session issuance, device refresh, API-token issuance, and account suspend/disable through one durable account revision write fence. Credential issue/refresh advances the account revision in the same transaction; stale concurrent lifecycle writes conflict, including when suspension has already enumerated credentials for revocation. Regression sources are added; execution remains deferred to the consolidated verification pass.
- [x] Recheck the persisted active-account state for every browser-cookie session/read/mutation path and at device authorization approval/token exchange, including session introspection, session listing, and API-token listing/issuance/revocation. Logout remains available so an inactive user can clear their session. Regression execution remains deferred to the consolidated pass.
- [x] Return the password version from the credential record that won the login CAS retry; a concurrent password reset can no longer issue a session bound to a stale version. Regression source updated; execution remains deferred to the consolidated pass.
- [x] Make account suspension/disable revoke persisted session families and API tokens in the same control-store transaction as the status change; serialize issuance against that transition. Persist recovery-triggered family and API-token revocations instead of changing only the process cache. Add regression sources; execution remains deferred.
- [x] Include all active API-token revocations in the same durable recovery-completion transaction as password reset, recovery-token consumption, and legacy session revocation; remove the post-commit token-revoke window. Execution remains deferred to the consolidated verification pass.
- [x] Consume every outstanding recovery token for the account atomically when one token completes; stale delivered links cannot reset the password after a newer reset. Regression source is written; execution is deferred.
- [x] Key recovery throttling by account for known identities so email/username aliases cannot each receive separate quotas; unknown identifiers remain hashed and enumeration-neutral. Regression source is updated; execution is deferred.
- [x] Make owner-initiated session revocation consume the REST idempotency key and atomically revoke the selected session plus its family; retry/replay behavior is implemented but remains unverified pending the consolidated pass.
- [x] Delete successfully consumed device authorization records and user-code indexes atomically; enforce expected-revision CAS for PostgreSQL control-record deletes and add stale-delete regression coverage. Source is updated; tests/build remain deferred to the consolidated pass.
- [x] Commit device-code consumption and exact-revision user-code-index deletion in the same transaction as access/refresh session and family creation plus account revision fencing. Auth accepts only a service-prepared consumption value. Failed session issuance no longer strands an approved code as consumed; concurrent exchanges race on the authorization revision. Added persistence regression source for commit and stale-delete rollback; consolidated verification remains deferred.
- [x] Add bounded session-family retention cleanup: keep expired session/refresh hashes for 30 days, retain device families through their refresh expiry, then remove child records and the family record atomically. The hosted cleanup worker runs this without adding scans to login requests; source regressions are added and deferred to the consolidated pass.
- [x] Zeroize one-time API-token, device authorization-code, access-token, and refresh-token HTTP DTOs after serialization/handling; redact device/user codes in desktop `Debug` output and zeroize desktop authorization response/pending values on drop. Source-only; verification deferred.
- [x] Keep API, recovery, and device-authorization token/code candidates in zeroizing owners until persistence succeeds; zeroize partial RNG buffers on both success and failure; redact service-level device-code `Debug` output. Source updated; consolidated verification remains deferred.
- [x] Zeroize the temporary `Set-Cookie` string after copying the required cookie into the response header, so session and CSRF values do not remain in an extra heap buffer. Source-only; consolidated verification deferred.
- [ ] Hosted PostgreSQL recovery/API-token restart E2E.
- [ ] Hosted webhook sink evidence and browser screenshot QA.
- [ ] Windows/macOS release evidence and one-pass production clone/reconciliation; repeated migration rehearsals are waived.

## Production recovery delivery policy

- [x] Keep outbound recovery delivery explicitly unsupported until a delivery provider is approved. Production configuration cannot enable webhook delivery; request/complete endpoints fail closed without creating or consuming recovery credentials.

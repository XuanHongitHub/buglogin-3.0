# Tasks

- [x] Add a domain invitation record and input/receipt DTOs: canonical email, workspace, role, expiry, issuer, token hash, revision, consumed/revoked state; no raw token persistence.
- [ ] Add durable invitation service operations for create, inspect, redeem, revoke, and accept, with constant-time hash checks, expiry, idempotency, audit, and workspace role authorization.
- [x] Resolve the authenticated account's verified canonical email at redemption; do not create account or credential records in the invitation flow.
- [x] Add REST endpoints and OpenAPI contracts for invitation create/redeem/revoke; use the existing self-accept `member.activate` operation after redemption and keep AccountId-based member invite backward compatible.
- [x] Register matching Tauri commands and typed desktop bridge; keep code-bearing create/redeem operations out of MCP.
- [ ] Add safe non-secret invitation operations to MCP only if the capability contract needs them.
- [ ] Wire the desktop member dialog to the canonical invitation API; remove direct password provisioning and any password-reset success path without a real API operation.
- [ ] Add API/domain persistence tests for expiry, replay, mismatch, role escalation, transaction rollback, enumeration resistance, restart persistence, idempotency, and secret redaction.
- [ ] Run generated-contract parity, typecheck, targeted API/domain tests, then authenticated Windows desktop invitation/acceptance smoke against PostgreSQL.

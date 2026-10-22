# MCP Idempotency Claim Release Plan

Status: implementation added; verification deferred to the user's requested consolidated pass.

- [x] Add owner-token-fenced abandon operation to in-memory and PostgreSQL control stores.
- [x] Release claims only for authentication/permission denials and the explicit serialized stale-authorization preflight in regular and async MCP dispatch.
- [x] Add storage release/reclaim and MCP failure-class regression source.
- [x] Document the claim lifecycle and safe failure boundary.
- [x] Require explicit control-store readiness, replay, claim abandonment, and advisory-lock implementations; remove success/501 defaults that could hide an incomplete adapter.
- [x] Require explicit remote-store readiness and checksum-conditional deletion; forward both capabilities in wrappers and test adapters.
- [x] Require every MCP execution policy to explicitly settle or abandon idempotency claims; remove the no-op success defaults.
- [x] Keep the MCP read snapshot gate at the transport boundary and let the executor own the mutation write gate; remove nested write-gate acquisition that rejected every MCP mutation.
- [x] Append MCP audit events to the control aggregate before persisting the idempotency completion, so restart cannot lose the event while retaining its receipt. Source change only; add restart/durability coverage in the consolidated verification pass.
- [x] Persist the denial audit event after safely abandoning an authorization-stale MCP claim, while keeping the key retryable. Source change only; cover restart durability and retry after authorization changes in the consolidated verification pass.
- [x] Persist standalone MCP denial and replay audit events, while keeping successful mutation audit events atomic with their idempotency receipt. Source change only; cover the audit paths across restart in the consolidated verification pass.
- [x] Hold the read snapshot gate through authorization/data execution, then release it before durable audit writes; persist query audit events separately from read-only idempotency receipts.
- [x] Keep the MCP control snapshot and mutation gates held through executor mutation, audit append, and idempotency completion; direct executor calls retain their own locking path. Source change only; add concurrent REST/MCP commit coverage in the consolidated verification pass.
- [ ] Run formatting and targeted storage/MCP tests during the consolidated verification pass.

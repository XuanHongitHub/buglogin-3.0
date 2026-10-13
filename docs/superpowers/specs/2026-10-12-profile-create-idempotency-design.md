# Local REST Profile Create Idempotency

Status: implementation in progress.

## Contract

- Local and managed REST `profile.create` must use the same command identity and common Rust control mutation path, with authenticated profile scope, validated ASCII idempotency key, mutation journal, and audit.
- Managed resource provisioning is an implementation detail of `profile.create`; it must not expose a second `profile.provision` idempotency namespace.
- Repeating the same key with the same normalized input replays the original `201 Created` response; reusing that key for different input returns `409 idempotency_conflict`.
- If the process stops after profile storage commits but before the idempotency receipt is durable, a retry may recover only when the existing profile exactly matches the requested profile; a mismatched existing ID remains a conflict.
- Hosted production persistence uses PostgreSQL; local authority mode uses its configured control store and recovers ambiguous completion by matching the resource record.
- A failed request without a valid key must not create a profile.
- Preserve the existing response schema and `201 Created` status.
- OpenAPI must describe the actual REST input and versioned response shape.

## Verification

- Local REST missing-key, successful create, exact replay, process-reopen recovery, changed-input conflict, and no-side-effect tests.
- HTTP/API, MCP, command registry, format, and diff checks.

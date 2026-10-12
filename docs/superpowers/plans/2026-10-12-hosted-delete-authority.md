# Hosted Profile Delete Authority Boundary

Status: safe failure behavior implemented locally; hosted deletion remains deferred.

1. [x] Route hosted REST and MCP deletion through one Rust-owned capability guard.
2. [x] Return one explicit `503 hosted_profile_delete_not_ready` response before touching local profile storage.
3. [x] Add hosted REST regression coverage while preserving local deletion behavior.
4. [ ] Implement hosted quarantine, object-retention cleanup, durable idempotency, and rollback before enabling hosted deletion.

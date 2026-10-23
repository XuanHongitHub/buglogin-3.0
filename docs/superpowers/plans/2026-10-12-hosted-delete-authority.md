# Hosted Profile Delete Authority Boundary

Status: hosted quarantine/delete and retention cleanup are implemented in the current worktree, but have not been verified or deployed. Keep production acceptance open until the deferred consolidated verification pass.

1. [x] Route hosted REST and MCP deletion through one Rust-owned capability guard.
2. [x] Keep local-authority deletion separate; the previous hosted `503 hosted_profile_delete_not_ready` guard was removed only after the hosted quarantine path was added.
3. [x] Add hosted REST regression coverage while preserving local deletion behavior.
4. [ ] Verify hosted quarantine, object-retention cleanup, durable idempotency, and restore/rollback behavior before production acceptance. REST and MCP now use the same Rust quarantine service; expired payload cleanup writes a durable journal and reuses the fenced sync-GC worker. Source implementation is present but unverified.

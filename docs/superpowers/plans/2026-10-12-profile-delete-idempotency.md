# Profile Delete Contract Parity

Status: local REST/MCP parity implemented and targeted gates pass; hosted deletion remains deferred.

1. [x] Require a validated idempotency key for local REST profile deletion.
2. [x] Route REST deletion through the shared Rust profile deletion service and control mutation journal.
3. [x] Keep MCP on the same service; recover a completed quarantine after reopening local storage without guessing.
4. [x] Test missing key, exact replay, changed-input conflict, storage-reopen recovery, and hosted fail-closed behavior.
5. [x] Run API, MCP, storage, contract, and format gates.

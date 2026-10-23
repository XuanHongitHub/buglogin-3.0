# Profile Create Idempotency Plan

Status: local and managed REST create now share idempotency/audit semantics; targeted API, MCP, OpenAPI, contract, and format gates pass.

1. [x] Route local and managed REST profile creation through the shared mutation journal and audit path under `profile.create`.
2. [x] Preserve `201 Created` and the versioned profile response shape for successful creation and replay; align OpenAPI.
3. [x] Cover missing key, exact replay, process-reopen recovery, changed-input conflict, and no side effect.
4. [x] Run targeted API, MCP, contract, and format gates.

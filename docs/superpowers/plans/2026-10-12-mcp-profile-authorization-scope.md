# MCP Profile Authorization Scope Plan

Status: implemented and locally verified.

1. [x] Resolve canonical workspace/resource bindings for hosted MCP profile actions.
2. [x] Route REST and hosted MCP profile authorization through one backend decision helper.
3. [x] Apply the shared decision to MCP profile list filtering.
4. [x] Preserve token-scope and explicit capability-deny precedence; restrict resource shares to read/execute.
5. [x] Add regression coverage for workspace grants, resource shares, token restrictions, destructive-action denial, and resource deny behavior.
6. [x] Route profile resource manifest/blob/runtime read and execute lease checks through the canonical profile authorization decision.
7. [x] Run sync authorization, REST profile, MCP execution/vertical, unit, and formatting gates.

Verification passed: `cargo test -p buglogin-api --lib` (62 passed, including the sync read/update authorization regression), `cargo test -p buglogin-api --test http_profiles` (8 passed), `cargo test -p buglogin-api --test http_sync_migration` (1 passed), `cargo test -p buglogin-api --test mcp_execution` (20 passed), `cargo test -p buglogin-api --test mcp_vertical` (5 passed), `cargo test -p buglogin-api --test compatibility` (4 passed), `pnpm check:api` (7 passed), `node --test tests/contracts/contract-drift.test.mjs` (7 passed), `cargo fmt --all -- --check`, and `git diff --check`.

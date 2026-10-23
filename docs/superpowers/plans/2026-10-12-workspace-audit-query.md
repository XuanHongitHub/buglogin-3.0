# Workspace Audit Query Plan

Status: Rust REST/MCP and web client/UI integration are implemented and targeted gates pass. Screenshot, keyboard, and accessibility QA remain open; production compatibility remains unverified until staging confirms the old bare-array response is not in use.

1. [x] Add bounded cursor pagination to the Rust audit service and REST handler, with deterministic newest-first ordering and validation.
2. [x] Register `audit.events.list` as a canonical REST/MCP read command; align schemas, OpenAPI, and generated adapter contracts.
3. [x] Add MCP execution through the same audit service and workspace scope.
4. [x] Add the typed web client and replace the audit placeholder with real paged records and API-derived states.
5. [x] Verify API authorization/pagination, MCP parity, contract generation, and web typecheck/tests. `cargo test -p buglogin-api --lib audit::tests` (2 passed), `cargo test -p buglogin-api --lib scope_tests::audit_events_list_uses_the_workspace_scope_and_shared_page_service` (1 passed), `cargo test -p buglogin-api --test http_control` (3 passed), `cargo test -p buglogin-api --test mcp_vertical` (5 passed), `pnpm --filter @buglogin/web test`, `node scripts/generate-api.mjs --check`, contract drift (7 passed), `cargo fmt --all -- --check`, and `git diff --check` passed. Screenshot, keyboard, and accessibility QA remain open.

Compatibility gate: confirm the deployed BugLogin v1 API has not shipped the previous bare-array audit response before staging rollout.

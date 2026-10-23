# MCP Command Scope Parity Plan

Status: implemented and locally verified.

1. [x] Map `team.create` to its explicit workspace scope.
2. [x] Add regression coverage for workspace-scoped authorization.
3. [x] Run MCP execution/vertical tests and contract generation checks.

Verification: `cargo test -p buglogin-api scope_tests::hosted_profile_get_uses_canonical_workspace_scope_for_authorization`, `cargo test -p buglogin-api --test mcp_execution` (20 passed), `cargo test -p buglogin-api --test mcp_vertical` (5 passed), `cargo fmt --all -- --check`, API generation check, and `git diff --check` pass.

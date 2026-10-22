# Profile Create Contract Unification Plan

Status: contract unification was locally verified; follow-up legacy-secret rejection is implemented in the worktree and awaits the consolidated verification pass. Production/staging release evidence remains separate.

1. [x] Add a shared domain `ProfileCreateInput` matching the canonical command schema.
2. [x] Normalize canonical and legacy REST/MCP payloads through one server-owned profile construction path.
3. [x] Keep hosted workspace, proxy, ACL, revision, and idempotency checks authoritative.
4. [x] Align desktop remote API payload and web client request/response tests.
5. [x] Align OpenAPI source and contract copy; generate the web `ProfileCreateInput` from the Rust OpenAPI source and check generated drift in the contract suite.
6. [x] Run the targeted API, desktop, web, generation, formatting, and diff gates.
7. [x] Route REST and MCP legacy profile normalization and workspace selection through the same Rust helper; verify identical precedence and fail-closed behavior.
8. [x] Reject secret-bearing legacy runtime configuration/fingerprint payloads consistently in REST, MCP, hosted migration import, and local package activation before profile persistence, using one domain security policy also used by migration inspection; it detects credential fields, bearer/basic values, and proxy URL/userinfo forms with or without a scheme. Verification is deferred with the current implementation batch.

Verification: `cargo test --locked -p buglogin-api` passes on Windows, including 73 API unit tests and all API integration suites; focused reruns also passed `http_profiles` (9) and `mcp_execution` (23). `pnpm test:contract` (10), `cargo fmt --all -- --check`, and `git diff --check` pass. The legacy conversion and workspace precedence are now shared by both REST and MCP. Existing unrelated dead-code warnings remain in browser and migration crates.

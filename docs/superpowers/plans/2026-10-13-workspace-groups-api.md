# Workspace Groups API Implementation Plan

## Status

Server integration verification passed for the implemented group CRUD slice. OpenAPI-derived web API methods are now wired and self-tested. Approved UI and hosted PostgreSQL restart evidence remain open; this plan does not mark the full capability complete.

## Phases

1. [x] Audit current group service, persistence, REST/MCP scope resolution, and generated contracts against the design.
2. [x] Add server integration coverage for REST authorization, workspace isolation, revision conflicts, idempotency, and MCP CRUD parity. Unit tests cover revision and delete-in-use.
3. [x] Run targeted API, MCP, OpenAPI, generation, and contract checks.
4. [x] Verify hosted control-aggregate reopen behavior with the shared control-store implementation.
5. [ ] Verify PostgreSQL process restart and wire the approved UI before accepting the full capability.
6. [ ] Record the UI and production-release evidence without claiming production readiness.

## Acceptance

- `cargo test --locked -p buglogin-api services::groups::tests`
- `cargo test --locked -p buglogin-api --test http_control`
- `cargo test --locked -p buglogin-api --test openapi_coverage`
- `node scripts/generate-api.mjs --check`
- Relevant MCP integration tests and `pnpm test:contract`

## Evidence recorded 2026-10-13

- `cargo test --locked -p buglogin-api --test http_groups`: passed; exercises REST CRUD, authorization, cross-workspace masking, revision conflict, and idempotent create replay.
- `hosted_group_state_survives_api_state_reopen` passed against the shared in-memory ControlRecordStore; this verifies aggregate reload behavior, not a PostgreSQL process restart.
- `cargo test --locked -p buglogin-api services::groups::tests`: passed; exercises workspace isolation, authorization, revision conflict, and delete-in-use.
- `cargo test --locked -p buglogin-api --test mcp_execution production_group_commands_share_workspace_acl_revision_and_idempotency_contracts`: passed; exercises MCP CRUD, workspace ACL, revision, and idempotent replay.
- `cargo test --locked -p buglogin-api --test openapi_coverage`: passed (3 tests).
- `profile_group_replacement_is_workspace_scoped_and_revision_safe` passes end-to-end through REST, covering the profile binding workspace, membership validation, CAS revision, idempotent replay, and stale revision rejection.
- MCP group/profile-group commands now share regression coverage for membership replacement, revision increment, idempotent replay, and clearing memberships before deleting the group.
- OpenAPI-derived group/profile-record DTOs and typed web reads/writes are generated from canonical schemas; the web self-test and generator check pass.
- `node scripts/generate-api.mjs --check`: passed.
- `pnpm test:contract`: passed (10 checks; 59 capabilities, 50 packets, 94 commands).
- `cargo test --locked -p buglogin-api`: passed after adding the six group/profile-group MCP schema fixtures and canonical action entries; 75 library tests and all API integration test targets passed.
- Hosted-mode test fixtures seed the canonical account record because hosted account provisioning is disabled by design; no auth bypass was added. PostgreSQL process restart evidence remains unverified.

Passing this plan verifies the server API slice only; it does not prove desktop/web feature parity, hosted sync infrastructure, PostgreSQL process restart behavior, or release readiness.

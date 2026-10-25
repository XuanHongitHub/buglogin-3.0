# Tasks

- [x] Confirm existing organization/workspace/account contracts and packet ownership.
- [x] Define organization membership entity, status, parent-gate semantics,
  and backward-compatible aggregate migration.
- [x] Add domain serialization, membership service, and focused permission tests.
- [x] Add REST list/provision/invite/status/workspace-assignment routes with audit,
  idempotency, scope checks, explicit deny, and token-scope enforcement.
- [x] Update OpenAPI, command registry, adapter contract, generated API contracts,
  MCP and Tauri dispatch.
- [x] Add organization-membership parent gate to workspace authorization
  paths; prove suspended users cannot reach child resources by REST/Tauri/MCP.
- [x] Add migration/backfill tests for existing owners, active/suspended
  workspace members, invitations, and accounts spanning multiple organizations.
- [x] Implement organization Members and Workspace Access pages using existing Donut
  components; keep create-account and invite distinct.
- [x] Run focused assignment, membership-service, MCP-schema-fixture, desktop
  typecheck, and desktop self-tests.
- [ ] Complete the full API suite and role matrix. The 3.0.9 command-registry
  integration attempt was blocked before test execution by MSVC LNK1318 while
  linking an unrelated admin binary; its 8 tests passed before the version bump.
- [ ] Complete screenshot/interaction QA and supported Windows/macOS smoke.
- [ ] Rehearse production migration and rollback; release only after Windows and
  macOS evidence.

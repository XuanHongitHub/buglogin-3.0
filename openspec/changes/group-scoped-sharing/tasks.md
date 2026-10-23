# Tasks

- [ ] Confirm group-target and action semantics in the shared 3.0 contract; document the legacy source-code limitation and mapping risk.
- [ ] Add a typed group-grant target and durable group-grant model with legacy-compatible profile-grant deserialization.
- [ ] Add group share/revoke/list service operations with workspace, active-member, revision, idempotency, and audit enforcement.
- [ ] Add one hosted profile-access evaluator that checks direct and dynamic group grants after token-scope and explicit-deny evaluation.
- [ ] Route list/read/execute/update/share protected profile operations through that evaluator; verify cross-workspace and moved-profile denial.
- [ ] Add REST and MCP registry/contract parity; add Tauri/web adapters over the same Rust service.
- [ ] Extend canonical migration records and importer to carry groups, group-target grants, revoked state, access mode, and source provenance.
- [ ] Reject ambiguous target/member/workspace mappings with row-level diagnostics; never silently discard or expand grants.
- [ ] Add tests for current and future group members, reassignment, revocation, full vs run-sync-limited actions, deny/token precedence, workspace isolation, persistence restart, migration dry-run/import/replay/rollback.
- [ ] Run API, domain, migration, desktop, web contract, and E2E gates; record evidence and residual limitations.

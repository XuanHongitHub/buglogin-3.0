# Organization Extension Defaults Plan

- [x] Extend `OrganizationPolicy` with a backward-compatible workspace-to-group mapping and regenerate contracts.
- [x] Validate mappings in organization updates and clear workspace mappings when a workspace is removed. Legacy migration import leaves this field empty because that format has no extension-group records.
- [x] Apply the organization default on new profile creation, with workspace policy precedence.
- [x] Add selectors to the existing Organization editor, loading groups per workspace and allowing stale selections to be cleared.
- [x] Add focused service regression coverage and run static contract, typecheck, desktop-test, and release-verifier gates.
- [ ] Complete Rust API tests and installed Windows/macOS smoke evidence; release remains pending those gates and the required default-branch workflow.

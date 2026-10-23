# Delegated resource administration implementation plan

1. [x] Confirm user-approved role/scope and large-resource ASCII direction; inspect current API/UI and mark unsupported actions.
2. [~] Replace local-only org/member/workspace mutations with scoped API calls; remove controls whose behavior has no safe supported endpoint.
3. [ ] Complete account-to-membership-to-role invite flow using existing authenticated APIs; keep identity creation separate from scoped access.
4. [ ] Design and implement versioned delegated-role/account APIs for organization, workspace, and team scope, including role capability ceilings and audit.
5. [~] Implement scalable resource inventory, group/profile assignment, access detail, and guarded bulk operations using real backend pagination and state.
6. [ ] Verify API ACL/revision/audit, frontend typecheck/tests, contract parity, accessibility, visual/interaction evidence, and Windows/macOS acceptance.

## Verification record

- User approved the ASCII roles and resource inventory direction in chat on 2026-10-23.
- Existing docs confirm org/workspace/member APIs exist, while PlatformAdminView and OrgManagementView contain local-only bulk role/status/quota, fake password reset, owner transfer, and workspace deletion behaviors.
- Existing profile group replace accepts an empty `groupIds` array; profile resource binding is workspace-scoped. Cross-workspace/org transfer has no supported command.
- Release gates are not satisfied: current worktree has unrelated pending changes, no current visual-evidence JSON/screenshots, and no Windows/macOS role/resource interaction acceptance.
- Current implementation pass: OrgManagementCenter now loads org/workspace/member records through the desktop API; member role/status and workspace policy changes call scoped API commands and refresh server state. Fake organization identity, fixed plan/quota claims, manager role option, and fabricated profile counts were removed from the live view. Unsupported password reset, ownership transfer, archive/delete, security-policy, and billing actions no longer report false success; workspace deletion/archive/password reset/security/billing APIs remain unsupported.
- `pnpm --dir apps/desktop exec tsc --noEmit` passes after making the shared public-entry TSX imports explicit.
- `pnpm --dir apps/desktop test` passed (model, bridge, and profile-workflow self-tests).
- `pnpm exec biome check --reporter=summary src/components/org-management-view.tsx src/components/resource-inventory-panel.tsx src/components/pending-invites-dialog.tsx` passed with no diagnostics.
- `pnpm --dir apps/desktop build` passes after making the shared public-entry TSX imports explicit and emits assets under `G:\buglogin-target\desktop-dist`; existing sourcemap, `use client`, mixed dynamic-import, and large-chunk warnings remain.
- Replaced the hard-coded fake pending invitation card in `PendingInvitesDialog` with an explicit state because there is no invitation inbox/list API.
- Added `ResourceInventoryPanel` with workspace selection, bounded 50-row cursor pagination, current-page filters, refresh, detail, and direct-grant inspection; added its approved ASCII reference at `docs/design/references/DES-ADM-02-resource-inventory.md`. The existing API does not expose team-inherited grant provenance, resource moves, or bulk resource mutations; the screen says so explicitly.
- Still required before this packet is complete: delegated account provisioning/membership assignment, Lead/custom role contract and Rust/MCP authorization, resource assignment/move and guarded bulk operations, tests for partial bulk failure/scope enforcement, visual/interaction evidence, and Windows/macOS install/update smoke. No release promotion is authorized by this implementation pass.

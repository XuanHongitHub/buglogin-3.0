# Organization membership implementation plan

Status: local implementation includes the organization membership model and
parent authorization gate, organization member REST/MCP/API paths, the
organization.member.workspace.assign command, and Members plus Workspace
Access UI. Targeted API and desktop checks pass. Full role-matrix, GUI/OS,
migration rehearsal, and production release acceptance remain open; no
production deployment is claimed.

- [x] Verify current source: organizations store owner account only; other
  members, invitations and status are workspace scoped.
- [x] Confirm requested hierarchy requires an organization-level parent gate,
  not a UI-only aggregate of workspace members.
- [x] Create synchronized OpenSpec proposal/delta/tasks and superpowers design.
- [x] Add backward-compatible `OrganizationMember` domain model and deterministic
  persisted-state backfill.
- [x] Add parent authorization gate to the central REST/Tauri/MCP execution path.
- [x] Add owner/Platform Admin organization member create/list/status API;
  preserve separate workspace invitation, assignment and account creation semantics.
- [x] Add existing organization-member workspace assignment through
  organization.member.workspace.assign, with same-organization and active
  parent checks, revision, audit/idempotency, and actor/role-ceiling enforcement.
- [x] Add API/OpenAPI/command registry/MCP/Tauri contract parity and audit.
- [ ] Complete role matrix for Platform Admin, Org Owner, Workspace Owner/Admin,
  Lead, Operator and Member, including explicit deny and multi-org isolation.
- [x] Implement organization Members and Workspace Access workflows in the
  existing UI.
- [ ] Capture screenshot/interaction evidence for both workflows, including
  loading, empty, forbidden, error and stale-refresh states.
- [x] Add focused assignment, membership-service, command-registry, MCP-schema,
  profile-count, and desktop state regressions.
- [ ] Complete the full candidate API suite, migration rehearsal, and
  Windows/macOS QA.
- [ ] Release only after both OS install/update and hosted API/sync gates pass.

## Current risk

Focused API tests cover Org Owner assignment, Workspace Admin denial, Owner-role
escalation denial, out-of-organization workspace denial, and suspended-parent
conflict. The organization member service verifies lower-role assignment and
role-ceiling denial. Desktop typecheck and self-tests pass; the MCP schema
fixture test passes on candidate 3.0.9. The full candidate API suite remains to
be rerun after the fixture fix; the 3.0.9 command-registry attempt was blocked
before execution by MSVC LNK1318 while linking an unrelated admin binary.

The profile-count issue is bounded by explicit unavailable state and the home
UI now associates profile rows/counts only with a successful snapshot for the
selected workspace. Main profile access behavior still needs GUI diagnosis.
Production migration rehearsal, complete role-matrix acceptance, screenshot
and interaction evidence, Windows/macOS acceptance, and release remain open.

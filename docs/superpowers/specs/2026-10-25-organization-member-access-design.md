# Organization membership and parent access gate

## Problem

The product contract requires `Organization → Workspace → Group/Profile` member
management. Current code has one owner account on each organization and stores
all other memberships at workspace scope. As a result, an organization cannot
create or suspend a member as one operation, and a disabled organization member
could still retain direct workspace membership.

## Domain decisions

- `Organization.accountId` remains the single organization owner and cannot be
  changed by this feature.
- Add a separate `OrganizationMember` entity with account, organization,
  status, and revision. The first version has no custom organization role: owner
  authority stays with `accountId`; other organization members receive workspace
  roles only through separate workspace membership and grant operations.
- Organization membership is a parent authorization gate. Suspended membership
  blocks all child workspace and resource actions but preserves child membership
  rows and grants. Reactivation restores only previously assigned access.
- Account identity may participate in multiple organizations. Suspension is
  isolated to one organization.
- Direct account provisioning and invitation remain different flows. Org-level
  creation creates an identity and org membership but grants no workspace access.
- Assigning an existing organization member to one workspace is a separate
  operation. The REST route is POST
  /v1/organizations/{organization_id}/workspaces/{workspace_id}/members and the
  canonical command is organization.member.workspace.assign. It accepts the
  organization member ID and expected parent revision and returns the created
  workspace Member.
- Existing persisted aggregates are backfilled deterministically from workspace
  memberships. Existing workspace role/status/grants are never rewritten.

## Authorization design

- Centralize the parent-gate check in the API authorization snapshot used by
  REST, Tauri/desktop adapters and MCP dispatch, before command execution.
- Treat Platform Admin as global override. For non-platform callers, resolve
  workspace → organization and require active organization membership, except
  for the organization's owner account.
- Validate account creation, role ceilings, explicit denies and token scopes
  before creating any identity, credential, member, invitation or grant.
- Restrict organization.member.workspace.assign to Platform Admins and the
  organization owner. The parent member must be active and current, and the
  workspace must belong to the requested organization. An organization owner
  may assign lower workspace roles (including Admin) but cannot grant Owner;
  Workspace Admins and Leads use the existing workspace member/invitation/role
  flows and their existing ceilings, not this organization-level command.
- Audit every organization member mutation with actor, organization, target,
  action, revision and correlation ID; never include passwords or invitation
  tokens.

## UI

- Organization detail has dedicated `Members` and `Workspace access` pages.
- Member creation and invitation remain separate full-page flows.
- Workspace access lists existing organization members and current workspace
  roles, and lets an authorized organization manager assign an active member to
  the selected workspace with an allowed role.
- Member detail shows workspace assignments and effective roles, with actions
  routed to the selected workspace for role changes.
- Organization suspend/reactivate is one audited parent action and displays the
  effective impact across the organization's workspaces before confirmation.

## Acceptance

- Domain/backfill, API authorization, REST/MCP/Tauri contract parity and account
  role-ceiling tests are implemented; the full candidate API suite remains an
  acceptance gate.
- Workspace assignment validates organization scope, active parent revision,
  actor authority, idempotency and audit. Verified cases include Org Owner
  assignment, Workspace Admin denial, Owner-role escalation denial,
  cross-organization workspace denial and suspended-parent conflict.
- The MCP schema fixture for OrganizationMemberWorkspaceAssignInput is valid on
  candidate 3.0.9. The full candidate API suite and complete role matrix remain
  acceptance gates.
- Suspended organization members cannot access child workspaces by any supported
  API path; unrelated organizations and Platform Admin remain functional.
- UI implementation covers create, invite, suspend/reactivate, workspace
  assignments, loading and errors. Screenshot/interaction evidence, full role
  matrix, forbidden and partial-failure review remain acceptance gates.
- Production migration has snapshot, dry-run, rehearsal and rollback evidence.

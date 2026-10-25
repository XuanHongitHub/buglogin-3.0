# Add organization membership and delegated workspace access

## Why

The current model stores only `Organization.accountId` as owner. All other
memberships and suspension are workspace scoped, so an organization owner has no
API operation to create, invite, list, or suspend a person at organization scope.
An organization screen that only aggregates workspace memberships cannot meet
the requested `Organization → Workspace → Group/Profile` access model.

## Product behavior

- Keep the existing organization owner as the account identified by
  `Organization.accountId`; ownership transfer remains a separate operation.
- Add an organization membership with `active` or `suspended` status. It does not
  itself grant access to a workspace resource. Workspace membership and grants
  remain required for workspace data and actions.
- A suspended organization membership denies access to every workspace and
  resource below that organization while preserving the account's access to
  other organizations. Reactivation restores the account's previous
  workspace-level permissions without changing their roles or grants.
- Organization owners and Platform Admins can provision or invite organization
  members, manage organization status, and assign lower-level workspace access.
  A scoped manager can only manage identities and roles below their effective
  grant ceiling.
- Account creation, invitation, organization membership, workspace membership,
  and workspace role assignment are separate, explicit operations. Creating an
  organization member does not silently grant access to every workspace.
- Platform Admin authority remains global and audited.
- The new organization-member workspace assignment command is restricted to
  Platform Admins and the organization owner. It assigns an active existing
  organization member to one same-organization workspace using the current
  parent revision. The organization owner may assign lower workspace roles but
  cannot grant Owner through this command; Workspace Admins and Leads retain
  their existing workspace-scoped member, invitation and role flows.

## Implementation scope

- Add backward-compatible domain and persisted control-state representation.
- Backfill active organization memberships for accounts already present in any
  workspace in that organization. Existing workspace roles/statuses and grants
  remain unchanged.
- Add authorized, audited, idempotent REST operations and OpenAPI schemas for
  organization member list, account provisioning, invitation, status, and
  workspace access assignment.
- Expose the same operations through Tauri and MCP command contracts and enforce
  authorization in Rust/API.
- Add organization-level Members and Workspace Access pages to the existing
  hierarchy. Keep invitations separate from direct account creation.

## Compatibility and migration

- Existing workspace memberships remain authoritative for workspace role and
  resource access. The new active organization membership is a parent gate.
- Old persisted control aggregates deserialize with an empty organization member
  collection; a deterministic migration backfills membership from existing
  organization owners and workspace memberships before serving requests.
- Existing account, member, workspace, and grant IDs and wire shapes remain
  compatible. New REST/MCP operations are additive.
- No production migration runs without snapshot, dry-run, rehearsal, verification,
  and rollback proof.

## Acceptance

- A Platform Admin can manage organization members across tenants; a scoped
  caller cannot read or mutate another organization.
- An organization owner can provision/invite a member, inspect organization
  members, suspend/reactivate a member, and manage workspace access beneath the
  organization.
- Suspending an organization member blocks REST, Tauri, and MCP access to all
  child workspaces while preserving other-organization access and stored grants.
- Reactivation restores only previously assigned workspace access; it does not
  add a role or grant.
- Workspace owner/admin/lead role ceilings, explicit deny, token scopes,
  idempotency, audit and partial failures are covered by API and role-matrix
  tests.
- Desktop screens have loading, empty, forbidden, error, stale-refresh and
  mutation-result states; screenshot and interaction evidence is recorded.

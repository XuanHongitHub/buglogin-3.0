# Organization membership

## ADDED Requirements

### Requirement: Organization membership is a parent access gate

The system SHALL store an organization-scoped membership for each non-owner
account attached to an organization. Membership status SHALL be `active` or
`suspended`. An active organization membership SHALL be necessary, but not
sufficient, for access to a child workspace. Existing workspace membership,
role and explicit grants remain required.

#### Scenario: Existing workspace account is backfilled

- GIVEN a persisted organization with an account that has a workspace membership
- WHEN the control state is upgraded
- THEN an active organization membership is created for that account
- AND the existing workspace membership, role, status and grants are unchanged

#### Scenario: Organization member has no workspace assignment

- GIVEN an active organization member with no workspace membership
- WHEN that account lists or opens child workspace resources
- THEN the API denies the workspace operation
- AND the organization member can still view organization-level membership data

#### Scenario: Organization member is suspended

- GIVEN an account with active organization and workspace memberships
- WHEN the organization owner suspends that account in the organization
- THEN all access to child workspaces in that organization is denied by REST,
  desktop commands and MCP
- AND stored workspace memberships, roles and grants are preserved
- AND other organizations containing the same account remain accessible

#### Scenario: Organization member is reactivated

- GIVEN a suspended organization member with preserved workspace assignments
- WHEN the organization owner reactivates that membership
- THEN the account regains only its prior workspace-level access
- AND no additional workspace, role or grant is created

### Requirement: Organization member administration is authorized and audited

The system SHALL allow Platform Admins and the organization owner to list,
provision, invite, suspend and reactivate organization members within authorized
scope. Lower-level managers SHALL be limited to their effective grants, scope,
and role ceiling. Every mutation SHALL validate its target at the API boundary,
be idempotent where retried, and create an audit record.

#### Scenario: Cross-organization target is denied

- GIVEN an organization owner for Organization A
- WHEN that owner reads or mutates a member in Organization B
- THEN the API returns a scoped denial without disclosing the target record

#### Scenario: Scoped manager cannot grant a higher role

- GIVEN a workspace Lead with permission to create Operator and Member accounts
- WHEN the Lead requests an Admin, Owner, or Platform Admin role
- THEN the API rejects the request and creates no account, membership, credential,
  invitation, grant or audit-success record

#### Scenario: Creation differs from invitation

- GIVEN an authorized organization owner
- WHEN the owner provisions an account directly
- THEN an account identity and organization membership are created without an
  invitation acceptance step
- AND no workspace access is granted unless explicitly assigned
- WHEN the owner invites an identity
- THEN a pending, expiring invitation is created and access is withheld until
  redemption

### Requirement: Existing organization members can be assigned to a workspace

The system SHALL expose an additive REST route at
POST /v1/organizations/{organization_id}/workspaces/{workspace_id}/members and
the canonical command organization.member.workspace.assign through REST, Tauri
and MCP. The request SHALL identify the existing organization member and its
expected revision, and the response SHALL be the created workspace Member.
Only Platform Admins and the organization owner may invoke this organization-
level assignment. The target organization member SHALL be active at the stated
revision, and the workspace SHALL belong to the same organization. Every
successful mutation SHALL be audited and idempotent.

The organization owner may assign lower workspace roles, including Admin, but
SHALL NOT grant the Owner role. Workspace Admins and Leads SHALL continue to use
the existing workspace member, invitation and role flows with their existing
ceilings; they SHALL NOT gain organization-level assignment authority.

#### Scenario: Organization owner assigns an active member

- GIVEN an active organization member and a workspace in that organization
- WHEN the organization owner assigns the member the Admin workspace role using
  the current organization-member revision
- THEN an active workspace membership is created and returned
- AND a retry with the same idempotency key returns the same result without a
  duplicate membership or audit mutation

#### Scenario: Workspace manager and role escalation are denied

- GIVEN a Workspace Admin or Lead who has no organization-owner authority
- WHEN that actor calls organization.member.workspace.assign
- THEN the API denies the request
- WHEN the organization owner attempts to assign the Owner role
- THEN the API denies the role escalation

#### Scenario: Parent or workspace scope is invalid

- GIVEN an organization member who is suspended or whose revision has changed,
  or a workspace belonging to a different organization
- WHEN an assignment is requested
- THEN the API rejects it and creates no workspace membership

Implementation tests currently cover Org Owner assignment, Workspace Admin
denial, Owner-role escalation denial, cross-organization workspace denial and
suspended-parent conflict. The complete role matrix and live REST/Tauri/MCP
parity acceptance remain open.

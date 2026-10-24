# Organization and Workspace Management Design

Status: implementation in progress
Date: 2026-10-24
Owner: IO-UI with existing IO-CONTROL contracts

## Goal

Make the existing native organization and workspace management capabilities discoverable through the hierarchy `Organization → Workspace → Groups / Profiles`, with account creation and invitations as separate, scoped workflows. Preserve API authorization as the authority and avoid representing workspace membership as an organization-wide membership.

The full role/scope, resource, account/invitation and page ASCII contract is maintained in `docs/design/org-workspace-access-resource-model.md`.

## Existing contract boundaries

- Organizations own workspaces; workspaces own memberships, groups, and profiles.
- Existing account provisioning creates an account and workspace membership. It is not an invitation and may leave login disabled pending credential verification.
- Invitations are a separate workspace-scoped flow.
- Existing suspend/role operations mutate workspace membership. The UI must label this scope accurately.
- Group/profile permissions are resource-scoped. Cross-workspace profile moves are not implied by group management.
- UI role checks are for discoverability only; server policies remain authoritative.

## User journey

1. Open Organizations and select an organization from a real API-backed list.
2. Open a workspace in that organization.
3. Manage that workspace's members, create an account, or invite an existing identity.
4. Navigate to groups and profiles within the same workspace; allow ungrouped profiles where the current contract supports it.
5. Return to the parent scope without losing the selected organization/workspace.

## Required behavior

- No fake org-level membership or suspension semantics.
- No account creation hidden behind a generic nested modal as the sole entry point.
- Keep create-account and invite flows distinct.
- Every list/detail has loading, empty, error/forbidden and refresh behavior using current Donut components/tokens.
- Mutations refresh the affected in-memory data from the API without requiring app restart or page reload.

## Out of scope for this slice

- New organization membership schema or endpoints.
- Cross-workspace profile transfer unless an existing atomic API is verified.
- Changes to release/deploy workflows, API cleanup helpers, or runtime/MCP behavior.

## Acceptance

- A manager can reach the correct authorized workspace and see an explicit member-management/account-creation entry point.
- Account creation uses the existing scoped API; invite uses the existing invitation API.
- Organization and workspace scope are visible throughout navigation.
- No frontend-only permission is treated as authorization.
- Targeted typecheck and tests pass; UI changes receive available interaction/screenshot QA before release.

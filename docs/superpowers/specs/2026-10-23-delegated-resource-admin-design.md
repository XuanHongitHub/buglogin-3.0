# Delegated resource administration and inventory UX

## Approved product behavior

- Platform Admin may administer global accounts, organizations, workspaces, memberships, roles, and resources, with audit.
- An Org Owner/Admin may create or invite identities into its organization and manage workspaces, members, roles, and resources only inside its organization authority.
- A Workspace Owner/Admin may create or invite accounts into that workspace and manage its members, roles, groups, profiles, and grants.
- A Lead is a scoped manager for its assigned team/resources. It may create/invite accounts into that scope and manage only subordinate memberships, roles, and resources it can administer.
- Operator and Member actions remain bounded by their effective role and explicit grants.
- Account creation creates an identity in a pending/invited state; it does not grant global access. Workspace/org membership and role assignment are explicit, scoped steps.
- Role administration is delegated: the assignable capability set and target scope must be subsets of the actor's authority. Platform Admin is never assignable by a scoped manager.
- All protected actions are authorized by Rust/API, audited, revision-checked, and consistent across UI, REST, Tauri, and MCP.

## Resource inventory UX

- Large resource sets use server-side search/filter/sort and cursor pagination or virtualization; never load/render the full tenant inventory.
- Keep organization/workspace/team context visible. Show resource type, name, group, state, effective access, sync/lease status, and last-updated time.
- Saved views and filters cover owned/shared, running, syncing, locked, and needs-attention resources.
- Bulk selection distinguishes current page from all matching results and previews permission/data effects before mutation.
- Resource detail uses tabs for overview, access, sync/data, and activity. Direct and group-derived grants display their source and effective result.
- Group assignment can include multiple groups or none. Cross-workspace/org move is a separate guarded transfer workflow; it must not be emulated by changing a group or workspace field.
- Encrypted group/profile membership changes surface key-rotation requirements and block sync until rotation completes.

## UI states and safety

- Preserve the inventory shell on loading/error; retain stale rows with a freshness indicator while refreshing.
- Provide first-use empty, filtered-empty, forbidden, offline/degraded, partial bulk failure, locked, conflict, success, and archived states as applicable.
- No local-only success, synthetic quota/identity, fake license, password reset, ownership transfer, archive, or delete behavior.
- If no authoritative API exists, remove the action or show the backend-derived unavailable reason and safe next action.
- Match Donut desktop components, density, tokens, keyboard/focus patterns, and responsive behavior.

## Current contract gaps

- Existing membership roles are Owner/Admin/Operator/Member; Team Lead and organization-scoped role definitions are not first-class domain roles yet.
- Custom role definition/assignment and delegated account provisioning need an approved versioned API/domain contract before exposing those actions as complete.
- Organization ownership transfer, account password reset/recovery delivery, workspace archive/delete, billing, and license operations have no supported domain endpoints and remain unavailable.
- Profile group replacement exists within one workspace, including an empty group list; cross-workspace/org profile transfer is not implemented.

## Acceptance

- Role matrix proves scope containment and no privilege escalation for platform, organization, workspace, team, group, profile, REST, desktop, and MCP callers.
- Real large-inventory interaction evidence covers search/filter/page selection/bulk partial failure and detail/access views.
- Windows/macOS screenshot, keyboard/focus, responsive, forbidden/offline/conflict states, and owner smoke evidence are required before release acceptance.

# Organization Extension Defaults

## Goal

Let an organization administrator choose a default extension group for each workspace. Profiles created in that workspace inherit the organization's choice only when the workspace has no explicit default. Extension groups remain workspace-owned.

## Behavior and authority

- Rust `OrganizationPolicy` stores `defaultExtensionGroupsByWorkspace`, keyed by workspace ID.
- The API validates each mapping against a workspace in the same organization and an extension group owned by that workspace.
- A workspace-level default overrides the organization mapping. If neither is configured, new profiles have no default group.
- Existing profiles are not changed when defaults change.
- Removing a workspace clears its organization mapping. If an extension group is removed later, the stale ID is surfaced in the editor so an administrator can clear it; new-profile creation reports the invalid configured default rather than silently hiding the configuration error.
- Organization edits use existing revision checks and organization permissions.

## UI

Extend the existing Organization editor with one compact row per workspace and a group selector populated from that workspace's groups. Include explicit loading, unavailable, empty, and save-error states. Use existing Donut-compatible Dialog, Select, and semantic tokens.

## Compatibility

The new policy field is optional on input and omitted when empty, preserving existing organizations and clients. REST/OpenAPI, generated desktop command contracts, and persisted organization policy preserve the mapping. Older organization policy JSON remains compatible. The legacy migration package has no extension-group records, so it cannot represent this default; importing that format leaves the field empty.

## Acceptance

- API tests reject cross-organization workspaces and cross-workspace groups.
- New profiles inherit the organization default; a workspace default wins.
- Organization UI can set and clear a mapping and reloads the persisted value.
- Existing policy JSON without the field still loads.

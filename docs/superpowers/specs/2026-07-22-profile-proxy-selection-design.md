# Profile Proxy Selection Design

Status: implementation wave.

## Authority

`Profile.proxy_id` is the only proxy-assignment authority. No separate assignment repository or dual write is introduced.

## Contract

- `profile.create` accepts optional `proxyId`.
- Hosted creation requires `workspaceId`; selected proxy must exist in that workspace and pass `CAP-NET-001` read authorization.
- Local creation requires the selected proxy to exist in the local Rust repository.
- Missing, cross-workspace, unauthorized, or invalid proxy IDs fail before profile/resource/manifest writes.
- Profile responses expose only `proxyId`; proxy credentials and credential references never enter profile payloads.
- Delete integrity remains enforced by scanning `Profile.proxy_id` references.

## UI

The existing desktop create drawer lists redacted proxy metadata for the selected workspace. `No proxy` remains explicit. Loading, empty, forbidden, and error states remain actionable.

## Acceptance

- REST, MCP, Tauri local, and production remote paths accept the same optional `proxyId`.
- Hosted cross-workspace and unauthorized selection fail closed without partial writes.
- Local missing proxy selection fails closed.
- Created profile preserves the selected proxy through restart and list/get responses.
- Desktop create flow uses real inventory data; no stub options.

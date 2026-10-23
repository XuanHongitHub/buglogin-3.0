# Desktop Group REST Parity Design

## Goal

Expose the existing workspace group and profile-group REST authority through the BugLogin desktop Tauri command surface. Reuse the Rust API routes, version checks, idempotency, audit, and workspace ACL. Do not create a second group store or duplicate business rules in the desktop client.

## Scope

- Add canonical Tauri adapters for `group.list`, `group.get`, `group.create`, `group.update`, `group.delete`, and `profile.groups.replace`.
- Route production desktop requests to the existing `/v1/workspaces/{workspaceId}/groups` and profile group assignment endpoint.
- Narrow command wrapper payloads to the OpenAPI request body and preserve idempotency keys for mutations.
- Add typed desktop bridge methods and transport tests for exact paths, methods, bodies, auth, correlation, idempotency, and result shape.

## Constraints

- REST remains the authority for hosted groups and profile membership. The desktop must not cache mutable group authority as an independent source of truth.
- Local-only mode cannot mutate hosted groups; it must fail explicitly if invoked without a hosted API.
- UI implementation is excluded. `DES-07` still requires a complete approved reference package and Donut interaction review under `docs/design/ui-authority.md`.
- Preserve group revision/CAS and profile revision/CAS semantics exactly as defined by the API.

## Acceptance

- Command registry, generated Rust/TypeScript contracts, adapter projection, and OpenAPI remain in sync.
- Production desktop invocation reaches each canonical REST route and serializes only the specified wire body.
- Local mode does not report a false success or persist an independent group copy.
- Relevant API contract checks, desktop tests, generation check, formatting, and diff checks pass.

# Workspace Audit Query Design

## Problem

The Rust API already exposes a workspace-scoped audit read route and authorizes it with `CAP-AUD-001`, but the route is absent from the canonical command registry and both OpenAPI documents. The web workspace audit page therefore claims that no query contract exists and never loads real events. The current endpoint also returns the entire matching history without a bound.

## Required behavior

- Add one canonical read command, `audit.events.list`, with REST and MCP adapters and the existing workspace-scoped `CAP-AUD-001` authorization.
- Return a stable, bounded page ordered newest first, with a cursor bound to `(occurredAt, eventId)`. Validate page size and cursor at the API boundary; never silently truncate or return unbounded history.
- Preserve the existing workspace ACL decision and audit redaction behavior. The cursor must not grant access to another workspace or event.
- Publish the input/output shapes in the command registry, generated adapter contract, and both OpenAPI documents. The Rust REST route, MCP tool, and web client must share those shapes.
- Replace the workspace audit placeholder with real API data, including loading, empty, forbidden, offline/error, and load-more states. Reuse current workspace design-system table and status components.
- Do not add audit mutation, export, support impersonation, or new persistence schema in this change.

## Compatibility

- The route path remains `GET /v1/workspaces/{workspaceId}/audit-events`; response changes from a bare array to `{items,nextCursor}` and therefore requires the v1 contract rollout to be deployed atomically with updated clients. Before production rollout, confirm no deployed client consumes the old array response.
- No stored audit-event schema changes. Existing event data remains readable; page ordering is deterministic by timestamp then event ID.

## UI reference

- Screen: `WSP-21 Audit`, workspace operator/admin context; primary job: investigate workspace actions.
- Authority: existing shared workspace design system, `DESIGN.md` audit timeline, `docs/design/ui-authority.md`, IA row, and screen-state matrix. No new shell or visual language.
- Required interaction evidence: real API fixture, page continuation, loading/empty/error/forbidden behavior, keyboard table navigation, and responsive screenshot.

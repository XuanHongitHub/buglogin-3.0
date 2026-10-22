# Member, Resource, and Grant List Pagination

## Decision

`member.list`, `resource.list`, and the new `grant.list.page` use the same bounded cursor-page shape across REST, Tauri, and MCP: `{ items, nextCursor }`. REST accepts optional `cursor` and `limit`; limit defaults to 100 and is capped at 100. Cursors are canonical IDs (member, resource, or grant respectively), at most 128 characters, and are applied as exclusive ID boundaries after workspace authorization.

The server remains the authority for authentication, workspace authorization, ordering, and cursor validation. Items are ordered by their canonical IDs. A cursor only advances within the already-authorized workspace collection. Clients follow `nextCursor` until null when they need the complete collection.

## Compatibility

The existing `GET /members`, `GET /resources`, and `GET /grants` array responses remain available for v1 compatibility. Canonical paginated command routes are `GET /members/page`, `GET /resources/page`, and `GET /grants/page`; existing routes and operation IDs keep their array response contracts. `grant.list.page` is the new page command, so existing REST and generated clients remain compatible. Desktop profile sharing uses member pages and grant pages until `nextCursor` is null. Mutation routes and item representations do not change.

## Acceptance

- Invalid or oversized page limits and malformed cursors return canonical 400 Problems.
- Each page contains no more than 100 items, is ordered deterministically, and has a valid continuation cursor only when more rows exist.
- Workspace ACL checks happen before returning any page.
- Grant pages retain optional resource/member filters and enforce workspace and sharing authorization before returning any page.
- REST, desktop command transport, and MCP return identical page semantics.
- List consumers that need all records follow every page and reject cursor cycles.

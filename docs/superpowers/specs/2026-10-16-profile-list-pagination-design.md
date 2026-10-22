# Profile List Pagination Contract

## Decision

`profile.list` is one bounded cursor-page contract across REST, Tauri, and MCP. Input accepts `includeArchived`, `limit` (default 100, maximum 100), an optional canonical profile ID cursor of at most 128 characters, and optional `workspaceId` for workspace-bound listing. Output is `{ items, nextCursor }`, ordered by profile ID. The cursor advances after the last returned, visible profile so authorization and archive filtering cannot expose hidden rows.

The Rust profile-listing service is the shared authority for hosted REST and MCP validation, workspace binding, archive filtering, profile-read authorization, and cursor semantics. Tauri's hosted adapter requests REST pages. Local Tauri and local API authority select only the next bounded set of entity IDs from the repository directory and deserialize those rows, instead of loading every profile record for each page.

The desktop convenience method follows pages, with cursor-cycle protection, when its caller omits page input and needs the full workspace view. Passing pagination input returns one page. Desktop uses the generated `ProfilePage` command type, whose rows include the revision required by revision-bound updates.

## Compatibility

Existing `profile.list` calls that omit `limit` and `cursor` continue to return the full visible profile set through the desktop convenience API. Direct command, REST, and MCP adapters use a bounded page with the default limit of 100. Profile record fields and mutation contracts do not change.

## Acceptance

- Limits outside 1..=100 and invalid profile cursors return canonical 400 Problems.
- REST rejects duplicate and unsupported query keys and malformed values with canonical Problem responses, matching MCP validation semantics.
- Pages are ordered by profile ID, contain at most `limit` authorized/non-archived rows, and expose a continuation cursor only when another visible row exists.
- REST, MCP, and Tauri page adapters preserve the same page semantics.
- Desktop full-list convenience calls follow every page and reject repeated cursors.
- Profile authorization is enforced before an item is returned or a cursor is emitted.

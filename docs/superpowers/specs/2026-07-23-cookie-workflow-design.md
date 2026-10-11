# Cookie Workflow Design

## Status

Design gate. Production implementation not approved until persistence and Wayfern materialization pass.

## Authority

The stopped profile's native Wayfern cookie database remains the local source of truth. Hosted sync stores authenticated profile snapshots/blobs, not a second independently mutable cookie ledger. `CookieService` remains a policy/secret helper until it has durable metadata and native materialization.

## Commands

| Command | Adapters | Input | Output | Rule |
|---|---|---|---|---|
| `cookie.list` | Tauri/REST/MCP | workspace + profile | metadata summaries | Never returns values |
| `cookie.import` | Tauri/REST/MCP | one-use staged import ID + CAS generation | count report | Profile stopped; lease/fencing; idempotent |
| `cookie.copy` | Tauri/REST/MCP | source/destination scopes + destination CAS | count report | Source read + destination create; both stopped |

No separate `cookie.clone`, `cookie.get`, or raw `cookie.export` command. Copy covers clone semantics. Export requires a later encrypted-download contract.

## Security invariants

- Raw values never appear in REST, MCP, audit, events, errors, filenames, IDs, or diagnostics.
- MCP accepts opaque staged-import references only.
- Cross-workspace copy requires independent source and destination ACL checks.
- Batch writes are all-or-nothing.
- Metadata and secret-vault versions commit transactionally.
- Revoked profile access invalidates list/copy immediately.
- Production never falls back to native local vault authority.

## Required states

Loading, empty, stopped-required, running-conflict, stale-generation, lease-lost, partial-input, success-count, rollback, forbidden, offline, and secret-redacted diagnostics.

## Migration

Migration imports the native `Cookies` object through the encrypted profile-data package. Structured cookie projection is optional verification metadata only: count, identity checksum, and post-launch presence; never values.

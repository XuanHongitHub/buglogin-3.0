# Profile Record Revision Contract

Status: implementation in progress.

## Contract

- Logical profile command results are a flat profile object plus its current `revision`.
- `profile.list`, `profile.get`, `profile.create`, `profile.clone`, `profile.archive`, and `profile.restore` return that same record shape through MCP and the desktop command bridge.
- The REST transport may retain its explicit `VersionedProfile` envelope; the desktop REST adapter converts it to the canonical flat command record at the transport boundary.
- REST profile listing returns `items` and `nextCursor`, with stable ID ordering and a validated page size; the desktop adapter follows every page before returning the command result.
- Every read or mutation result that may be used for a revision-checked mutation must expose the current revision.
- Profile payloads continue to omit proxy credentials and secret references.

## Verification

- Assert revisions on local and managed MCP get/create/archive results.
- Assert revisions on desktop adapter create/list/archive/restore results.
- Run command registry generation checks, API/MCP/desktop tests, OpenAPI coverage, and format checks.

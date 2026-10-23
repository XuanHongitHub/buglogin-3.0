# Profile Read Page Contract Design

## Context

The Rust REST route `GET /v1/profiles` returns `VersionedProfilePage`, whose records have `schemaVersion`, `revision`, and `value`. The web client previously parsed the response as an array and its self-test mocked that incorrect shape. The OpenAPI `Profile` schema also omitted most fields serialized by the Rust domain entity.

## Contract

- `GET /v1/profiles?cursor=<id>&limit=<1..100>` returns `{ items: VersionedProfile[], nextCursor: string | null }`.
- Each versioned profile contains positive `schemaVersion` and `revision` plus a complete serialized `Profile` value.
- `Profile` fields and enum values match the Rust domain model. Nullable Rust `Option` fields are present as JSON `null` in responses.
- The web table requests one bounded page at a time and uses the opaque `nextCursor`; it does not fetch the entire workspace inventory eagerly.
- TypeScript profile, enum, page, and create-input types are generated from the Rust OpenAPI source. The docs OpenAPI copy is synchronized to that source.

## Failure behavior

- Invalid page limits fail before network I/O.
- Malformed page, version, or profile records are rejected as API response errors rather than silently accepted as empty data.
- An empty page with `nextCursor: null` is an empty result; previous/next navigation is disabled at the corresponding boundary.

## Scope

No REST route behavior, authorization, cursor ordering, or persistence semantics change. This aligns the client and contract with the existing Rust response.

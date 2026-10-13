# MCP Input Contract Design

Status: approved implementation target; runtime currently exposes incomplete schemas.

## Problem

`tools/list` currently returns `{ type: "object", additionalProperties: true }` for every MCP tool. This does not describe the command inputs and lets client integrations send arbitrary fields. The canonical command registry names an input schema, but several MCP command inputs do not yet have a matching OpenAPI component schema. The command dispatcher also relies on per-handler parsing instead of one verifiable schema boundary.

## Contract

- Every MCP tool advertises the exact input JSON Schema for its canonical `CommandSchema`.
- Input schemas come from the existing canonical API contract source and are generated deterministically; no hand-maintained MCP-only copies.
- Every command enabled for MCP resolves to a complete input schema. Generation fails on missing or ambiguous schemas; runtime never falls back to an unrestricted object.
- Object inputs reject unknown fields. Required fields, nullability, formats, and bounds match Rust request validation.
- The same schema is applied at the MCP boundary before authorization and execution, while domain services retain authoritative validation and authorization.
- Secret input fields may be marked write-only but remain accepted by the server; secret output rejection remains unchanged.
- REST and Tauri behavior, command IDs, scope resolution, ACL, idempotency, audit, and existing response shapes remain unchanged.

## Acceptance

- Every command whose adapter list contains `mcp` has one resolvable input schema.
- No advertised schema is the unrestricted `{type: object, additionalProperties: true}` fallback.
- Valid fixtures for each schema pass; missing required fields, wrong types, out-of-range values, and unknown fields fail before executor invocation.
- Generator check and contract drift tests detect stale or missing schemas.
- MCP execution and transport tests prove schema validation, ACL ordering, audit, idempotency, and secret handling remain intact.

## Scope boundary

Do not enable additional MCP commands, including hosted sync import, as part of schema parity unless their backend execution path already uses the canonical service and their idempotency/audit behavior is proven. Schema completeness must not be used to imply feature completeness.

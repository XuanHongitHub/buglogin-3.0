# REST Contract Parity Design

## Problem

Some control-plane OpenAPI operations describe a different method or omit the request/response contract implemented by the Rust router. Authenticated migration-control routes were also outside the canonical REST command registry. This lets generated frontend DTOs and API clients drift while basic registry tests stay green.

## Decision

Treat the Rust route behavior and its canonical OpenAPI document as one release contract. For the member-role route, document the actual `PUT`, required `{ role }` body, idempotency header, and `Member` response. Migration control import and rollback are canonical REST-only commands; they require capability authorization and idempotency and must describe their actual neutral-record DTOs and receipts. Generate every named OpenAPI component schema for TypeScript so REST-only request/response DTOs do not need handwritten duplicates. Keep route-specific composition types in the web client only when they do not duplicate a wire schema.

Neutral migration v1 records and their typed `data` objects reject undeclared fields. Forward-compatible values belong in the explicit `extensions` map; silently ignored nested properties are not accepted as successful migration input. API code reuses the canonical migration DTOs instead of defining a duplicate profile-resource wire type.

## Acceptance

- Contract test fails if the member-role method, body, response, or idempotency requirement drifts.
- Generator emits the named `MemberRoleUpdateBody` wire type from OpenAPI.
- Web API signatures use generated REST body types.
- Every authenticated Rust HTTP route has a canonical OpenAPI method and schema, or is explicitly deferred with a reviewed boundary.
- Control import documents the dry-run-plan vs activation-receipt response; rollback documents its confirmation and receipt.
- HTTP regression tests inject unknown properties at record, source-reference, data, workspace-policy, and nested profile-data levels and prove import rejects before mutation; OpenAPI mirrors the strict Serde behavior while keeping extension values open.
- Contract/generator checks and web typecheck/self-tests pass.

## Boundaries

No UI changes, migration execution, or production deployment are included. The migration import DTO behavior is intentionally strict at the v1 boundary because the production domain does not currently serve these v1 routes.

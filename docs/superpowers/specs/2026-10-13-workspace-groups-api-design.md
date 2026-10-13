# Workspace Groups API Design

## Goal

Provide durable workspace-scoped group CRUD and profile group reassignment through the canonical Rust API authority. REST and MCP must share the same service behavior, authorization capability, revision checks, audit trail, and persistence semantics.

## Invariants

- A group is visible and mutable only in its owning workspace; cross-workspace access is indistinguishable from not found.
- Group and profile mutations require `CAP-PROF-003` and use the existing mutation/idempotency/audit path.
- Updates use optimistic revision checks. Group deletion fails while a profile in that workspace references it.
- Replacing a profile's group IDs validates every group against the profile's workspace and writes the profile through its canonical revisioned repository.
- MCP and REST are adapters over the same Rust service and state transition; neither owns duplicate business rules.
- Persisted aggregate changes survive restart. No frontend-only success or in-memory-only production state.

## Scope

This contract covers the hosted workspace group service, REST routes, MCP commands, registry/OpenAPI/adapter contract parity, and server-side tests. UI integration is a separate packet and must follow `docs/design/ui-authority.md` and the approved Donut reference package.

## Failure behavior

Return explicit authorization, not-found, validation, revision-conflict, or in-use errors. Never silently drop unknown group IDs or overwrite a concurrent profile update.

# Desktop Group REST Parity Plan

Spec: `docs/superpowers/specs/2026-10-14-desktop-group-rest-parity-design.md`.

## Execution

- [x] Add Tauri adapter metadata for canonical group CRUD and profile group assignment.
- [x] Add production REST mappings with strict path validation and exact request bodies.
- [x] Add typed bridge methods and explicit local-mode failure behavior.
- [x] Add loopback coverage for route, method, body, auth, correlation, idempotency, and response shape.
- [x] Run desktop, API contract, generator, format, and diff gates.

## Evidence

- `cargo test --locked --lib -p buglogin-desktop-app`: 94 passed.
- `cargo test --locked -p buglogin-api --test command_registry --test http_groups --test mcp_execution`: passed (35 tests).
- `pnpm --dir apps/desktop test` and `pnpm --dir apps/desktop typecheck`: passed.
- `pnpm check:api` and `pnpm test:contract`: passed (24 checks; 101 commands).
- Local group commands fail with `hosted_workspace_authority_required` (409); no duplicate local group store is introduced.
- DES-07 UI remains separate because its approved reference package and visual/interaction evidence are still absent.

## Out of scope

Do not build the DES-07 UI in this packet. Its approved reference package and visual/interaction evidence are separate required gates.

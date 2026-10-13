# REST Contract Parity Plan

- [x] Inspect the Rust member-role route, request type, response serializer, and current web caller.
- [x] Correct the canonical OpenAPI method and wire schemas; mirror the contract copy.
- [x] Correct activation/suspension HTTP methods and mutation receipts.
- [x] Generate all named OpenAPI component DTOs and consume the role body type in the web API.
- [x] Document account administration, workspace policy reads, profile binding, and resource-grant wire shapes.
- [x] Add protected control-migration import/rollback to the REST-only command registry and document exact request/response schemas.
- [x] Generate snake_case wire fields as valid TypeScript identifiers without renaming API properties.
- [x] Add contract drift coverage and run generator, contract, and web checks.
- [x] Run Rust OpenAPI coverage and record final evidence/remaining production blockers.
- [x] Close nested migration-import contract drift: deny unknown fields in neutral v1 records/data, preserve arbitrary values only in each explicit `extensions` map, and reuse the migration crate's canonical profile-resource DTO instead of a duplicate API-local type.
- [x] Close profile lifecycle body drift: archive/restore require an explicit revision in both local and hosted paths, delete requires the documented revision plus quarantine confirmation, and Rust/OpenAPI reject unknown body fields.

## Evidence

- `pnpm generate:api` — pass.
- `pnpm test:contract` — 19/19 contract tests pass; capability/command registry check passes (100 commands).
- `pnpm --filter @buglogin/web test` — HTTP/router self-tests and TypeScript check pass.
- `cargo test --locked -p buglogin-api --test openapi_coverage --test http_control_import` — 4/4 pass.
- The migration HTTP test verifies `401` for invalid sessions and `403` without `migration.write` on both routes, and verifies strict top-level unknown-field rejection is returned as canonical `application/problem+json` with status/code `422` for both import and rollback.
- `git diff --check` — pass (Git reports existing LF/CRLF normalization notices only).
- Activation/suspension parity confirmed against the Rust router: both now use `PUT` and document idempotency plus the serialized `Member` receipt.
- Rust router inventory now has no undocumented HTTP route; import and rollback are registered as REST-only commands and both OpenAPI documents match the router contract.
- The Rust canonical-action assertion includes all 100 registry commands, including the device cookie inbox and REST-only migration control import/rollback.
- Neutral v1 nested record schemas reject unknown fields at every record/data boundary; forward-compatible vendor data belongs in the explicit `extensions` map. The local HTTP test injects extra fields into each entity family and nested profile/workspace values, then confirms the request fails before mutating imported accounts. This strict behavior is now the canonical API contract; no external `/v1` production route is currently served, so there is no deployed v1 consumer to preserve.
- Follow-up verification: `cargo test --locked -p buglogin-api --test http_control_import --test openapi_coverage` — 4/4 pass; `cargo test --locked -p buglogin-migration` — all tests pass; `pnpm generate:api`, `pnpm test:contract` — 21/21 plus canonical registry validation pass; `git diff --check` passes.
- `ControlImportInput` now uses the migration crate's exported `NeutralProfileResourceRecord` instead of a second API-local definition, keeping profile-resource wire parsing canonical.
- Profile lifecycle verification: `cargo test --locked -p buglogin-api --test http_profiles --test openapi_coverage` — 12/12 pass. HTTP coverage checks missing archive body, unknown archive/delete fields, explicit revision handling, hosted archive/restore replay, and delete quarantine fail-closed behavior. OpenAPI route coverage passes against the canonical document.
- Profile lifecycle request DTOs now reject unknown fields; local archive/restore no longer infer the current revision when the caller omits it, so stale writes cannot bypass compare-and-swap. `ProfileRevisionBody` and `ProfileDeleteBody` are documented in both OpenAPI copies.
- `cargo test --locked -p buglogin-api --lib body_dtos_reject_unknown_fields` — 2/2 pass; `pnpm test:contract` — 22/22 plus canonical registry check (59 capabilities, 50 work packets, 100 commands); `cargo fmt --all -- --check` and `git diff --check` pass (Git emits only existing LF/CRLF notices).

Hosted API behavior and production deployment remain outside this local contract gate and are still unverified.

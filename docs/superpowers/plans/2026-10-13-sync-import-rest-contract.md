# Sync Import REST Contract Plan

Status: implemented and locally verified; hosted staging and large-export transport evidence remain open.

- [x] Add closed serde input DTOs for the complete sync import payload tree.
- [x] Document exact request, receipt, and Problem wire schemas in both OpenAPI copies.
- [x] Add nested-unknown-field and OpenAPI drift regression coverage.
- [x] Replace Axum's 2 MiB default body limit with a documented 320 MiB cap; authenticate before reading, allow one direct import per process, and return canonical size/media/JSON/capacity errors.
- [x] Regenerate typed frontend API artifacts and run Rust/contract gates.
- [x] Record staging and transport limits without implying production sync readiness.

## Evidence

- `cargo test --locked -p buglogin-api --lib sync_import_rejects_unknown_fields_at_every_request_boundary` — pass; unknown fields are rejected at the import body, export, manifest, blob, and revision boundaries; omitted `dryRun` retains its documented `false` default.
- `cargo test --locked -p buglogin-api --lib sync_import_body_limit_returns_canonical_problem` and `... direct_sync_import_admission_is_bounded_and_retryable` — pass; over-limit requests return canonical 413 and saturated admission returns retryable 503.
- `cargo test --locked -p buglogin-api --test http_sync_migration` — pass; existing verified blob staging and authoritative manifest activation remain intact.
- Direct sync import now authenticates before consuming request bytes, accepts a 3 MiB body for DTO validation, rejects unauthenticated requests before JSON parsing, and returns 415 for unsupported media types. The body is capped at 320 MiB; 64 MiB objects serialized as JSON byte arrays can exceed the former Axum 2 MiB default.
- A per-process single-flight admission slot bounds memory pressure from direct JSON imports; saturation returns a retryable 503. A unit test proves saturation and permit release.
- `cargo test --locked -p buglogin-api --test openapi_coverage` — 3/3 pass, including runtime OpenAPI byte parity.
- `pnpm generate:api` and `pnpm test:contract` — pass; contract suite 23/23 and canonical registry check (59 capabilities, 50 work packets, 100 commands).
- `cargo fmt --all -- --check` and `git diff --check` — pass.
- No hosted staging request was made. JSON byte-array transport remains bounded by server request limits and has no large-export throughput evidence; production sync is still not proven.

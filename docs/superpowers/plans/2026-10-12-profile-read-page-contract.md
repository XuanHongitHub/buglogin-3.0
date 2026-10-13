# Profile Read Page Contract

Status: implemented locally; production release remains blocked by broader API, sync, and platform evidence.

1. [x] Make the Rust OpenAPI `Profile` schema match the serialized domain entity and close unknown fields.
2. [x] Define the versioned profile record and cursor page response in the runtime OpenAPI source.
3. [x] Generate frontend profile DTOs from the OpenAPI source and bind generated output to the source hash.
4. [x] Parse bounded cursor pages in the web client and expose page navigation in the profile view.
5. [x] Cover runtime response shape, cursor/limit requests, and contract drift.
6. [x] Keep `docs/contracts/openapi-v1.yaml` byte-identical to the runtime OpenAPI source.
7. [x] Run web typecheck/tests, contract gates, and focused Rust OpenAPI/profile/command-registry tests.

Verification on 2026-10-12:

- `pnpm --filter @buglogin/web typecheck`
- `pnpm --filter @buglogin/web test`
- `pnpm test:contract`
- `cargo test --locked -p buglogin-api --test openapi_coverage`
- `cargo test --locked -p buglogin-api --test http_profiles`
- `cargo test --locked -p buglogin-api --test command_registry`
- Runtime and documentation OpenAPI SHA-256 values match.

Residual: this closes the profile read-page mismatch only. It does not prove all REST/MCP/Tauri command schemas are generated from one complete source or close hosted PostgreSQL/S3, process-loss, migration, or platform release gates.

# Profile Create Contract Unification

## Problem

The command registry and desktop adapter define `ProfileCreateInput` as a small command DTO (`name`, `runtime`, optional runtime version/workspace/proxy). REST and MCP instead deserialize a full `Profile`. The web form sends the small DTO, so the real REST endpoint rejects it for missing entity fields. Existing web self-tests mock the response and do not expose the mismatch.

## Contract

- One shared Rust `ProfileCreateInput` is used by REST, MCP, and the Tauri adapter.
- REST and MCP construct server-owned profile state, timestamps, and defaults from that DTO; callers cannot set `state`, `archived`, timestamps, or other server-owned entity fields.
- Preserve existing full-Profile REST payloads during v1 migration, but normalize them through the same validation/provisioning path. Do not allow legacy fields to bypass profile, workspace, proxy, or idempotency checks.
- A requested profile ID is optional. If absent, derive a stable ID from the workspace and validated idempotency key so retries are deterministic.
- Hosted mode requires managed provisioning and an authorized workspace. Local mode creates a local profile. Proxy assignment is validated against the selected authority before writing.
- REST and MCP resolve legacy input and workspace selection through the same Rust application service. Workspace precedence is explicit input, REST's workspace header, then the sole authorized workspace in managed mode; the selected workspace is still checked by backend authorization before writes. Ambiguous managed scope fails before profile construction or side effects. Local mode does not infer workspace scope.
- Keep REST/OpenAPI, MCP, Tauri, web types, and generated command metadata aligned; contract tests must exercise a real Rust route using the web DTO.

## Verification

- REST tests cover canonical web DTO creation, same-key replay, changed-input conflict, hosted workspace/proxy authorization, legacy full-Profile compatibility, and rejection of invalid/forged state.
- MCP and Tauri profile-create tests pass.
- Web self-test asserts the serialized request body and verifies the API response shape from the real contract fixture.
- Run API, desktop Rust, web test/typecheck, OpenAPI/contract generation, formatting, and diff checks.

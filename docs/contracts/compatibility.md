# Contract compatibility

WP-47 treats `docs/contracts/command-registry-v1.yaml` as the canonical command inventory. REST, Tauri, MCP, and generated frontend metadata must not define a second command authority.

## Domain versions

- `domain-schema-v1.json` remains the immutable bootstrap contract for existing profile/group/proxy/runtime consumers.
- `domain-schema-v2.json` adds organization/workspace/team/member/resource/grant plus VPN and extension migration authority.
- VPN configuration uses a vault-backed `configRef`; extension bytes use a checksum-bound `artifactRef`. Inline secrets or binary payloads are incompatible.
- Readers reject unknown major versions. Migration may translate v1 into v2, never silently reinterpret v2 as v1.
- Organization/workspace inventory is hosted-authority data. Production desktop reads it through the Rust HTTPS adapter; the standalone local repository does not fabricate empty lists for that hosted-only surface.

## Drift rules

- REST `operationId` values must exactly equal commands declaring the `rest` adapter.
- Tauri command IDs must exactly equal commands declaring the `tauri` adapter; invoke names replace dots with underscores.
- MCP tool IDs must exactly equal commands declaring the `mcp` adapter; tool names replace dots with underscores.
- Tauri and MCP input/output schema names must equal the canonical registry metadata.
- `crates/api/openapi/openapi-v1.yaml` must expose the same REST command set as `docs/contracts/openapi-v1.yaml`.
- `frontend/src/generated/command-registry.ts` must retain the canonical registry SHA-256 and command ID union.

## Checks

```text
node --test tests/contracts/contract-drift.test.mjs
cargo test -p buglogin-api --test compatibility
pnpm check:api
```

## Current Gate 4 evidence

The previous 2026-07-20 drift list is stale. On 2026-10-12, the current worktree passed all listed interface-parity checks:

- `cargo test -p buglogin-api --test compatibility` — REST OpenAPI, Tauri commands, MCP tools, and generated client compatibility (4 passed).
- `pnpm check:api` — canonical registry, REST OpenAPI, Tauri, MCP, and generated Rust/frontend artifacts (7 passed).
- `node --test tests/contracts/contract-drift.test.mjs` — REST OpenAPI, adapter mappings, and generated client metadata (7 passed).

These checks prove command/interface parity for the checked artifacts. They do not prove hosted staging behavior, end-to-end role semantics, production sync readiness, or Windows/macOS release readiness; those remain separate gates.

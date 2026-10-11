# Contract compatibility

WP-47 treats `docs/contracts/command-registry-v1.yaml` as the canonical command inventory. REST, Tauri, MCP, and generated frontend metadata must not define a second command authority.

## Domain versions

- `domain-schema-v1.json` remains the immutable bootstrap contract for existing profile/group/proxy/runtime consumers.
- `domain-schema-v2.json` adds organization/workspace/team/member/resource/grant plus VPN and extension migration authority.
- VPN configuration uses a vault-backed `configRef`; extension bytes use a checksum-bound `artifactRef`. Inline secrets or binary payloads are incompatible.
- Readers reject unknown major versions. Migration may translate v1 into v2, never silently reinterpret v2 as v1.

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

## Current Gate 4 blockers

As of 2026-07-20, the checks intentionally report existing upstream drift rather than changing contracts outside WP-47 ownership:

- Canonical REST OpenAPI omits `migration.import`, `migration.inspect`, and `proxy.parse`.
- Runtime REST OpenAPI exposes only `system.health.get` and `proxy.parse`.
- Tauri adapter mappings omit eight commands declared for Tauri.
- MCP adapter mappings omit five commands declared for MCP.

Gate 4 interface parity remains blocked until the owning packets update their OpenAPI and adapter contracts. WP-47 must not baseline or suppress these differences.

# Tauri Local API Contract Gate

## Context

The desktop development launcher currently accepts an API when `/v1/readyz` is healthy and persistence is durable. A stale API can satisfy both checks while omitting operations required by the desktop, producing runtime failures only after the window opens.

## Decision

Before launching Tauri, fetch and parse `/v1/openapi.yaml` from the selected loopback API. Require the direct desktop login route plus the minimum account, organization/workspace, profile, proxy, and paginated-grant operations used by the production desktop flow. Reject malformed or incomplete contracts with a diagnostic naming each missing method/path/operation ID. This is a development startup guard, not a substitute for deployment verification or runtime request validation.

Allow selecting another explicit loopback HTTP origin through `BUGLOGIN_DEV_API_BASE_URL` so a source-current API can run beside an older API still held by an open desktop process. Reject remote hosts, credentials, non-HTTP schemes, paths, queries, and fragments. Allow a separate C:-drive Cargo cache through `BUGLOGIN_DEV_CARGO_TARGET_DIR`; never place development build cache on F:.

All supported desktop development entry points must route through this readiness and OpenAPI gate. In particular, root `pnpm tauri dev` and the desktop package's `tauri:dev` alias must use the local API launcher; they must not fall back to the Rust local-filesystem authority by default. Build commands continue to use the normal Tauri CLI path.

## Verification

Unit tests cover a complete contract, the canonical source OpenAPI document, missing required operations, and invalid input. A read-only check against the currently running local API rejects its stale OpenAPI because it lacks `GET /v1/workspaces/{workspaceId}/grants/page` (`grant.list.page`), even though its readiness and SQLite persistence checks pass. The helper reports this contract gap in under half a second, before Tauri starts. No process was stopped or changed.

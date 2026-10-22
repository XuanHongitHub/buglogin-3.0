# Tauri Local API Contract Gate Plan

Spec: `docs/superpowers/specs/2026-10-19-tauri-local-api-contract-gate-design.md`.

- [x] Add the required-operation contract validator and use it in the Tauri local API launcher before starting the desktop process.
- [x] Add tests for accepted canonical source contract, stale/missing operations, alternate loopback ports, invalid origins, and F: cache rejection — 8/8 passed.
- [x] Run the launcher contract tests, read-only `--check` against the current API, and `git diff --check`. The live `--check` correctly rejects `127.0.0.1:12344` because its OpenAPI omits `GET /v1/workspaces/{workspaceId}/grants/page` (`grant.list.page`); `/v1/readyz` is insufficient proof of desktop compatibility. Contract mismatch now fails fast. The active API and Tauri processes were left untouched.
- [x] Permit `BUGLOGIN_DEV_API_BASE_URL` to select an explicit loopback API origin and `BUGLOGIN_DEV_CARGO_TARGET_DIR` to select a separate build cache so source-current desktop dev can coexist with an older running API without replacing its locked executable; reject non-loopback origins and F: cache paths.
- [x] Start the source-current API on `127.0.0.1:12345` with isolated C: SQLite state; verify launcher `--check`, direct desktop login, and authenticated account lookup while leaving the existing desktop/API processes untouched.
- [x] Make port 12345 the launcher default because it is the source-current local API; leave `BUGLOGIN_DEV_API_BASE_URL` available for explicit loopback overrides. Verify default `--check` against the live contract and rerun all 9 launcher contract tests.
- [x] Route root `pnpm tauri dev` and the desktop package's `tauri:dev` alias through the local API readiness/OpenAPI gate, while preserving build command dispatch. Forward Tauri dev flags without a shell and add a regression for the root command mapping.

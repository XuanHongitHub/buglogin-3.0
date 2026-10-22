# Desktop Login Transport E2E

## Goal

Prove the source-current Rust desktop API client sends direct email/password sign-in to the canonical unauthenticated desktop-login REST route and safely parses the device-bound token response.

## Scope

- Exercise `RemoteApiClient::desktop_login` over a loopback HTTP server.
- Assert method, route, correlation header, and exact JSON request fields.
- Assert canonical response parsing and redacted token-bearing `Debug` output.
- Use synthetic credentials only. Do not access the OS vault, launch Tauri, or call a hosted API.

## Boundary

This proves the Rust HTTP transport contract only. API process E2Es cover server auth and workspace behavior; this does not prove the full Tauri UI, OS-vault persistence, or workspace mount.

## Desktop render regression found during E2E

The local Tauri WebView must render the sign-in screen without an uncaught React exception. The Vite browser smoke is insufficient because it does not exercise the WebView's startup path. For TanStack v9's compatibility hook, renderer callback context exposes state as `table.state`; code in callback contexts must not assume the outer compatibility wrapper's `getState()` method is present.

The local API desktop adapter must preserve the response envelope expected by existing Tauri commands. In particular, `group_list` returns `{ items, nextCursor }` even though the underlying HTTP endpoint returns a bare array; the compatibility command consumes `items`.

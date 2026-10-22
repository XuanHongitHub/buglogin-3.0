# Desktop Login Transport E2E Plan

Spec: `docs/superpowers/specs/2026-10-19-desktop-login-transport-e2e-design.md`.

- [x] Add a loopback HTTP test for the unauthenticated Rust desktop-login request/response contract. It checks POST path, correlation ID, exact credential/device DTO, token parsing, and token redaction.
- [x] Run the focused Rust test without rebuilding the API or launching a desktop window: `CARGO_TARGET_DIR=C:\buglogin-target cargo test --locked -j 1 -p buglogin-desktop-app --lib remote_api::tests::desktop_login_sends_direct_credentials_and_parses_device_session -- --exact --nocapture` — 1 passed.
- [x] Confirm the full existing API process E2Es remain the separate server-side evidence: login/CRUD/refresh, workspace isolation, and encrypted cross-device sync each passed on the source-current API binary on 2026-10-19.
- [x] Re-run all three process E2Es against `C:\buglogin-e2e-source-current` with `BUGLOGIN_E2E_SKIP_BUILD=1`; login/CRUD/refresh replay, workspace A↔B isolation, and encrypted cross-device profile/proxy sync passed. The API in `C:\buglogin-target` is stale and returns `405` for `/v1/workspaces/{id}/grants/page`; do not use it as source-current evidence or rebuild over the live dev process.
- [x] Run `cargo test --locked -j 1 -p buglogin-api --test http_auth -- --test-threads=1` on target `C:\buglogin-e2e-source-current` — 15 passed. An initial parallel run under concurrent local E2E load hit the intentional retryable password-verification capacity limit (`credential_capacity_exhausted`, 503); the isolated recovery test and serialized suite pass.
- [x] `git diff --check` passes for the touched files. `rustfmt --edition 2024 --check apps/desktop/src-tauri/src/remote_api.rs` still reports pre-existing formatting differences elsewhere in that already-dirty file; it reported none in the new test block. No whole-file formatting rewrite was applied.
- [x] Fix the Tauri WebView blank-screen regression found while reopening the app: TanStack v9 legacy header context lacks the subscribed state, so the name sort indicator now closes over component sorting state.
- [x] Fix the local `group_list` response envelope expected by the compatibility command (`items` plus `nextCursor`).
- [x] Verify the native Tauri WebView renders the BugLogin workspace with no uncaught JavaScript exceptions after both fixes. The first WebView inspection exposed the React crash and group response mismatch; after fixes, the DOM contains the workspace shell and the WebView reports no uncaught exceptions.

## Remaining boundary

This closes the Rust HTTP transport contract only. It does not prove Tauri UI submission, OS-vault persistence, workspace UI mount, hosted adapters, or browser runtime launch.

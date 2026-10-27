# Authoritative Profile Status Implementation Plan

1. Extend the existing runtime projection REST surface with an authorized `syncing` transition and desktop transport method.
2. Publish `syncing` only after the local browser process tree has stopped and before cloud sync work; retain `stopped` as the successful completion state.
3. Update lock resolution to consume explicit projection state. Preserve confirmed locks across transient status-read failures and clear only after a successful empty lease response.
4. Update command/API contracts and localized status text only where required by existing UI patterns.
5. Run focused formatting/typecheck/build and inspect the final diff against the already-dirty `main` worktree; do not overwrite unrelated WIP.

6. Replace per-profile binding, lease, and runtime reads with one bounded workspace status batch; authorize each profile, limit the batch to 250 profiles, and cap concurrent lease reads at 32.

Status: implementation complete; frontend typecheck, generated-contract checks, API/browser Rust compilation, and focused Windows regression tests pass. Production deployment and multi-instance runtime behavior are outside this local implementation evidence.

Verification update — 2026-10-27:

- `cargo check -p buglogin-api --lib` and `cargo check -p buglogin-browser --lib` pass on the current worktree.
- `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` passes using the already-built migration helper as a temporary Tauri resource; the temporary file was removed and verified absent afterward.
- `cargo test --locked -p buglogin-browser --test independent_chromium --test runtime_contract` passes (8 independent Chromium identity tests and 2 Wayfern contract regression tests). These tests do not replace the adapter plan's native browser smoke.
- Stop/sync lifecycle publishes `syncing` immediately after the stopped browser's lease and current manifest are confirmed, before key/proxy preflight. Idle-profile sync still publishes after acquiring its lease; already-synced cleanup retains `stopped`.
- Desktop typecheck and Vite build pass; build emits existing source-map, `use client`, mixed-import and large-chunk warnings.

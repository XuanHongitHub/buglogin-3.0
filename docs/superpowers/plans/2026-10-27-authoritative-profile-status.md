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

Lease recovery regression fix — 2026-10-28:

- The profile runtime previously detected an exited browser only when the UI requested `active_profiles`. If the UI was hidden and stopped polling, the heartbeat kept renewing the lease as though the browser were still running. That exit path also omitted `sync_started_at`, so its five-minute stopped-sync hold limit could never expire.
- The heartbeat now reconciles process exits itself, records the stopped-sync start time, stops the proxy bridge, and lets the existing bounded hold release the lease while preserving retry intent and local files.
- A stopped/pending-sync entry with a missing start time is treated as expired instead of renewing indefinitely.
- Windows regression test `heartbeat_detects_closed_browser_and_bounds_its_sync_lease` passes. It closes the managed process without a UI poll, verifies the heartbeat detects it, then verifies the lease is released after the hold deadline.

Sync lifecycle follow-up — 2026-10-28:

- A five-minute safety release could race an upload that was still actively syncing a large profile. The runtime now renews leases during an active sync attempt; the five-minute cap applies only while stopped and waiting for retry.
- Sync status is published only after key/proxy/recipient preflight and immediately before snapshot upload. Failed sync attempts publish `stopped`, retain dirty local files, queue retry, and release the held lease promptly.
- A managed browser stop starts sync inline. If that attempt fails, automatic reconciliation retries from local data using exponential backoff (3 seconds doubling up to 30 seconds), without holding a lease between attempts. Unexpected browser exit is detected by the desktop reconciler within about two seconds, then follows the same path.
- Sync duration has no fixed limit/ETA: it varies with changed profile data, object count, and network throughput. Each completed attempt now records end-to-end duration and success/retry in the local app log; production timing remains unmeasured until an instrumented build is exercised.
- The profile-list amber dot now uses the local sync event/API status only. A stale server runtime projection that still says `syncing` after its final stopped update failed no longer keeps the dot amber or implies a held sync.

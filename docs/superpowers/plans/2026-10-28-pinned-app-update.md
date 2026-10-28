# Pin prepared desktop updates to their checked release

Spec: `docs/superpowers/specs/2026-10-28-pinned-app-update-design.md`

## Status

- [x] Trace `app_update_release_changed` to the restart-time latest-feed check.
- [x] Confirm Tauri `Update` retains the exact version, URL, signature, and
  install context and verifies downloaded bytes against its captured signature.
- [x] Store that update object with prepared bytes and install from the captured
  object without rechecking mutable stable metadata.
- [x] Preserve signature checks, shutdown ordering, and retry state on failure.
- [x] Run Rust formatting and Windows desktop crate compilation; inspect the
  final diff.
- [ ] Bump/release only after the current 3.0.23 source fix is verified and the
  applicable release gates pass.

## Root cause

`restart_application` re-fetches stable `latest.json` and compares its current
version with the already downloaded package. Stable metadata can move while a
user is reviewing the restart prompt, causing a valid prepared update to fail.

## Verification

- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check` — pass.
- `cargo check --locked --manifest-path Cargo.toml -p buglogin-desktop-app`
  with `CARGO_TARGET_DIR=G:\\buglogin-target` — pass; pre-existing desktop
  dead-code warnings remain.
- `git diff --check` — pass.
- No app update was installed in this verification; the check validates the
  Windows Rust build only.

# Wayfern adapter hardening plan

Spec: `docs/superpowers/specs/2026-10-28-wayfern-adapter-hardening-design.md`

## Status

- [x] Inspect current runtime contract, local launch path, managed launch path,
  fingerprint controller, and ownership scope.
- [x] Verify the external Wayfern binary distribution/usage restriction.
- [x] Make local fingerprint setup/application failures non-blocking and return
  a clear warning while preserving `Running` state.
- [x] Replace free-port probing with Chromium port-zero allocation and strict
  `DevToolsActivePort` discovery.
- [x] Add focused tests for marker parsing, port-zero arguments, and launch
  behavior on fingerprint setup failures.
- [x] Run focused Rust formatting and desktop runtime tests.
- [x] Record remaining external authorization and platform runtime gates.

Verification: `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml
--check`, `git diff --check`, and `cargo test --manifest-path
apps/desktop/src-tauri/Cargo.toml --lib wayfern -- --nocapture` pass (16 tests).
The tests use mocked process/CDP endpoints; no real Wayfern binary was launched.

Release gate: Wayfern's official terms require written authorization to run its
binary outside Donut Browser. Obtain and record that authorization before
shipping a build that provisions or launches Wayfern.

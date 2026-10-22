# Tauri Native Updater — Execution Plan

Spec: `docs/superpowers/specs/2026-10-22-tauri-native-updater-design.md`.

Status: **active; release blocked pending credentials and full platform evidence.**

## Phase 1 — Replace fake updater behavior

- [x] Replace hard-coded version/ready responses with a fail-closed signed updater check/install path in the legacy command surface.
- [ ] Add Rust regression coverage that an unconfigured dev build reports a typed unavailable result and never claims an update is downloaded.
- [x] Add the Tauri v2 updater plugin to the active desktop and initialize it in the Tauri builder.

## Phase 2 — Signed feed generation

- [x] Inject the updater public key and HTTPS endpoint through the signing-build config without writing the private key into the repository.
- [x] Require the protected Tauri updater private key in signing jobs and collect platform updater bundles/signatures.
- [x] Assemble a version-bound `latest.json` for Windows x64, macOS arm64, and macOS x64; reject missing or inconsistent inputs.
- [x] Publish downloadable prerelease candidates and add a protected stable promotion workflow that verifies checksums/feed and changes only release metadata.

## Phase 3 — Client integration and verification

- [x] Expose Rust-authoritative check/download/install commands and progress/ready events.
- [x] Mount the existing update prompt hook in the active desktop entrypoint under the existing UI component.
- [ ] Add Windows/macOS clean-install and previous-version update E2E, invalid-signature rejection, interruption recovery, and rollback-channel checks.
- [ ] Update release policy/docs and close the release gate only after keys and platform evidence exist.

## Current evidence and constraints

- Official Tauri v2 updater uses mandatory artifact signatures, a pinned public key, and HTTPS in production. Its signature format is distinct from the existing Ed25519 evidence manifest; the latter is not a valid install trust root.
- Source integration and release-feed assembly are implemented. This does not prove the Rust plugin code compiles or that a released app can upgrade.
- `pnpm --filter @buglogin/desktop typecheck` currently fails on existing UI/API DTO and missing-symbol errors across cookies, extensions, profiles, proxy, workspace/member, and group-sharing components. The updater entrypoint introduces no diagnostic in the reported set; those UI paths must be reconciled by IO-UI against canonical Rust/API contracts, without inventing DTO fields.
- The current `E:\buglogin-target` has only about 30 MiB free. No Rust/Tauri compile or app launch was started; clean the one approved E: build target before the next native gate, and keep C: and F: out of build output.
- Local release credentials are absent. GitHub signing environments/keys and actual Windows/macOS update E2E remain unverified.

## Verification record — 2026-10-22

- `node scripts/tauri-cargo.mjs check -p buglogin-desktop-app --all-targets --target x86_64-pc-windows-msvc` — passed after cleaning the sole E: target; updater plugin and active Tauri command code compile for Windows.
- `pnpm release:verify` — 13/13 passed; updater config/feed/workflow tests — 18/18 passed; `actionlint` passed for release, signing, and stable-promotion workflows.
- `pnpm --filter @buglogin/desktop typecheck` — blocked by 80+ existing IO-UI contract and missing-symbol errors. Do not add unsupported API fields to make these pass.
- No Windows installer, macOS artifact, signed update, installed-app upgrade, staging rollout, or GitHub release has been verified.
- Do not build locally until the single-target cleanup prerequisite is resolved; do not write build outputs to C: or F:.
- Release credentials are not available in the local checkout. This blocks artifact publication, not source work.

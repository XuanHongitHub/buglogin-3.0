# Tauri Native Updater — Execution Plan

Spec: `docs/superpowers/specs/2026-10-22-tauri-native-updater-design.md`.

Status: **active; release blocked pending API parity, native platform artifacts, install/update evidence, and final publication.**

## Phase 1 — Replace fake updater behavior

- [x] Replace hard-coded version/ready responses with a fail-closed signed updater check/install path in the legacy command surface.
- [ ] Add Rust regression coverage that an unconfigured dev build reports a typed unavailable result and never claims an update is downloaded.
- [x] Add the Tauri v2 updater plugin to the active desktop and initialize it in the Tauri builder.

## Phase 2 — Signed feed generation

- [x] Inject the updater public key and HTTPS endpoint through the signing-build config without writing the private key into the repository.
- [x] Require the protected Tauri updater private key in signing jobs and collect platform updater bundles/signatures.
- [x] Assemble a version-bound `latest.json` for Windows x64 and macOS arm64; reject missing or inconsistent inputs. Intel Mac is excluded because the official Wayfern manifest has no macOS x64 runtime.
- [x] Wire immutable RC artifacts and a separate RC updater feed for BugLogin 3.0 to the existing DigitalOcean Spaces storage namespace `releases/desktop/v3/`; stable promotion publishes a separate stable feed after validating the selected artifacts.

## Phase 3 — Client integration and verification

- [x] Expose Rust-authoritative check/download/install commands and progress/ready events.
- [x] Mount the existing update prompt hook in the active desktop entrypoint under the existing UI component.
- [ ] Add Windows/macOS clean-install and previous-version update E2E, invalid-signature rejection, interruption recovery, and rollback-channel checks.
- [x] Update static release policy and infrastructure/updater documentation for the S3 layout; keep the release gate open until credentials and platform evidence exist.

## Current evidence and constraints

- Official Tauri v2 updater uses mandatory artifact signatures, a pinned public key, and HTTPS in production. Its signature format is distinct from the existing Ed25519 evidence manifest; the latter is not a valid install trust root.
- Source integration and release-feed assembly are implemented. The feed assembler and verifier accept immutable Spaces URLs, and the workflows upload RC/stable feed metadata separately. Static source checks do not prove that Actions can write/read the bucket or that a released app can upgrade.
- Release target is `XuanHongitHub/buglogin-3.0`; the previous `keyduc91/Malvanut-Login` findings are legacy 1.0 evidence only. Apple, S3, and Tauri updater secret/variable names have been configured in the 3.0 repo. The updater key is unencrypted; no password secret is needed. Secret values remain outside source and logs.
- The repo's current source is version `3.0.0`; the native Windows desktop `cargo check -p buglogin-desktop-app --target x86_64-pc-windows-msvc` passed using `G:\buglogin-target`. `pnpm release:verify` passes 15/15. An authenticated DigitalOcean Spaces preflight uploaded a disposable object under the isolated v3 preflight prefix, downloaded it publicly, matched SHA-256, deleted it, and confirmed it was no longer readable. This validates current write/read/cleanup credentials, not a workflow release.
- Windows Authenticode is not required by the user's manual-installer policy. The local `.github/workflows/signing.yml` patch removes that requirement; Windows Tauri updater signatures remain required for in-app updates. No Actions signing run, installer, macOS artifact, app update, or release publication has been verified.
- Direct production check with `node scripts/release/verify-staging-api.mjs` fails: 94 paths / 100 operations served vs source 110 / 122; 23 operations missing, including `auth.desktop.login` and `runtime.catalog.get`; runtime-catalog readiness header absent. `/v1/runtime/catalog` returns 404. API agent confirmed this is deployment drift: current source has the routes, while production serves an older binary or a different routed service. The API is reachable but cannot serve the 3.0 desktop contract.
- Public `releases/desktop/v2/rc/latest.json` and `stable/latest.json` checks are historical pre-3.0 observations. BugLogin 3.0 publishes under `releases/desktop/v3/`; a disposable write/download/checksum/delete preflight passed, but no release artifact/feed has been published.

## Verification record — 2026-10-22

- `cargo check -p buglogin-desktop-app --target x86_64-pc-windows-msvc` with `CARGO_TARGET_DIR=G:\buglogin-target` — passed; updater plugin and active Tauri command code compile for Windows.
- `pnpm release:verify` — 15/15 passed; `git diff --check` passed (line-ending warnings only). Actionlint and full package/sign/notarize builds still need current-run evidence.
- `pnpm --filter @buglogin/desktop typecheck` — passed on the current worktree after the About updater UX change.
- No Windows installer, macOS artifact, signed update, installed-app upgrade, S3 upload, staging rollout, or GitHub release has been verified. The existing 1.0 Spaces catalog remains separate and unchanged.
- The sole local/hosted Windows build target is `G:\buglogin-target`. `cargo clean` removed the old exact BugLogin targets on C: (8.7 GiB) and E: (108.5 GiB); both are empty. Do not start new builds on C:, E:, or F:.
- GitHub repo `XuanHongitHub/buglogin-3.0` now contains the configured updater private-key secret and `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY` variable; Apple/S3 secret names are configured there as well. GitHub will not return saved secret values. The local workflow removes Windows Authenticode requirements. macOS signing/notarization and release artifact/update evidence still need native Actions runs.

## Runtime catalog and current Windows package — 2026-10-22

- The current official Wayfern manifest reports `155.0.8059.79`, with Windows x64 ZIP and Apple Silicon macOS DMG. It does not publish macOS x64. Both packages were downloaded to G staging, inspected, SHA-256 hashed, and entered in a BugLogin Ed25519-signed catalog; the existing v154 Windows entry remains in that catalog for compatibility. The Mac DMG contains `Wayfern.app/Contents/MacOS/Wayfern`.
- `BUGLOGIN_RUNTIME_CATALOG_SIGNING_PRIVATE_KEY` is configured in the 3.0 repo. The corresponding public trust key and refreshed catalog still need to be committed and included in a rebuilt app before clients can verify v155.
- The signed catalog and public trust list have been installed under `/var/lib/buglogin3/runtime-catalog` on the isolated BugLogin 3.0 VPS. Two path variables were added to `/etc/buglogin3/api.env`, with a root-only backup retained. The running API has not been restarted or updated yet; live catalog publication remains pending the attested API deployment.
- The first local Windows release package is still compiling under `G:\buglogin-target`; it must be rebuilt after the public trust key/catalog source update before it can be treated as current release evidence.
- The first local Tauri v2 Windows build produced the signed NSIS installer `.exe` and `.exe.sig`, with no `.nsis.zip`. The release workflow/feed had incorrectly expected the legacy archive suffix. They now consume the actual v2 `.exe` plus detached `.exe.sig`; static release checks pass 15/15 and actionlint passes. The local artifact was built before the v155 trust-map update and is not the release candidate; a fresh build is required.

## Windows frontend embedding regression — 2026-10-23

- [x] Root cause confirmed from the installed 3.0.1 screenshot and Tauri's Windows path parsing: the release wrapper passed absolute `G:\\buglogin-target\\desktop-dist` as `frontendDist`, which Tauri interprets as a `G:` URL, leaving the app pointed at the build directory.
- [x] Keep Vite output in `CARGO_TARGET_DIR`, stage its files temporarily to the ignored `apps/desktop/dist`, and configure Tauri with relative `../dist` so the HTML and assets are embedded. Cargo-check helper also uses relative `../dist`.
- [ ] Run focused release policy/Cargo helper checks, build a Windows installer, install and launch it, and verify the BugLogin UI rather than a directory listing.
- [ ] Record a new immutable version/tag and workflow run. Do not promote stable; macOS notarization, OS update smoke, and Packet C/D gates remain release blockers.

## Development-build updater behavior — 2026-10-22

- Root cause of the About toast `app_updater_not_configured`: local/debug builds intentionally do not embed the Tauri updater public key, so the Rust command correctly reports that updater checks are unavailable. The UI previously surfaced this expected dev state as an error.
- Opening About no longer triggers a manual check; the user can press **Check for updates** to request one. That action shows a localized informational message for `app_updater_not_configured`; other network/feed/signature failures remain errors. Startup checking suppresses only the expected unconfigured-dev log. Rust also skips the three-hour background task when no updater key is embedded, and `build.rs` tracks updater key/endpoint environment changes to avoid stale Cargo artifacts.
- Verification: desktop TypeScript typecheck, Biome lint with formatting disabled, Rust format check, Windows desktop `cargo check --all-targets`, and `git diff --check` passed. Native interaction in the running desktop has not yet been visually rechecked.

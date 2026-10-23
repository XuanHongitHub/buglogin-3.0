# Single-Drive BugLogin Build Cleanup — Execution Plan

Spec: `docs/superpowers/specs/2026-10-19-single-drive-build-cleanup-design.md`.

Status: **active; legacy target roots are absent and supported build routing points to E:, but the current E: target is live and no clean rebuild has been performed.** This plan is a prerequisite for continuing the broader production fast-MVP plan at `docs/superpowers/plans/2026-10-19-production-fast-mvp.md`.

## Phase 0 — inventory and stop processes

- [x] Re-read current `git status`; preserve staged, unstaged, and untracked source work.
- [x] Recheck the known legacy target paths on C:, F:, and duplicate E: roots. Those paths are absent; `E:\buglogin-target` is the only confirmed active Rust target and is not a reparse point.
- [x] Verify the source-configured paths: Cargo target, Vite cache/output, Tauri build `frontendDist` override, and migration sidecar all resolve under `E:\buglogin-target` on Windows.
- [ ] Record full per-drive build-output size/free-space inventory. The stale `F:\Herd\buglogin\apps\desktop\dist` directory remains present, but the supported Tauri build wrapper does not consume or write to it.
- [ ] Stop only verified BugLogin build processes before a clean rebuild. API, Vite, and Tauri dev processes currently run from E: and remain active for local development.

**Gate:** inventory is complete, exact paths are reviewed, and all related processes are stopped.

## Phase 1 — clean build output only

- [x] Verify the known old build roots are absent: `C:\buglogin-target`, `F:\BugLogin-api-gate-20261019\debug`, `F:\Herd\bug-login\src-tauri\target`, `E:\BugLogin3Target`, and `E:\bug-login\src-tauri\target`.
- [ ] Resolve the stale `F:\Herd\buglogin\apps\desktop\dist` directory under the existing cleanup review decision. Do not retry a rejected deletion or route around policy.
- [ ] Preserve the live `E:\buglogin-target`; do not erase it while API/Tauri processes are using it.

**Gate:** all verified old build caches are absent; no C:/F: build target remains; E: has capacity for the clean build.

## Phase 2 — establish one build target

- [x] Update local `.cargo/config.toml` and `scripts/tauri-dev-local-api.mjs` defaults to share `E:\buglogin-target`.
- [x] Fail fast before Cargo starts when a Windows target resolves to C: or F:.
- [x] Update launcher contract coverage and `CODEX_CHAT.md` execution rules for the single target and prohibited drives.
- [x] Source inspection confirms Cargo target, Vite cache/output, Tauri bundle `frontendDist`, and migration sidecar use `E:\buglogin-target` through the supported Windows scripts.
- [ ] Build once and confirm the emitted Cargo/Tauri artifacts resolve to that exact path.

**Gate:** contract checks prove one target path and reject C:/F: before compilation.

## Phase 3 — clean build and local production-flow verification

- [ ] Build the source-current API and desktop exactly once into the canonical E: target.
- [ ] Start the API with PostgreSQL TLS plus the signed runtime catalog and trust-key paths; verify readiness reports PostgreSQL and catalog availability and catalog GET returns the signed Wayfern entry.
- [ ] Launch the rebuilt desktop through the standard local API launcher; verify the app renders, signs in using the existing local admin, creates a profile, installs the catalog runtime through the managed installer, and opens the profile.
- [ ] Record build commands, hashes/versions, test output, process paths, target size, drive free space, and remaining production gates in the master production plan.

**Gate:** the app opens a browser profile from current source, with no BugLogin build output created on C: or F:.

## Rollback

Cleanup is limited to verified compiled build output, so rollback is a fresh rebuild from source into the canonical target. Do not delete profile data, database files, runtime archives, installed browser runtimes, signing material, or source. If the clean build fails, retain its single E: target for diagnosis and do not restore duplicate caches.

# Single-Drive BugLogin Build Cleanup — Design

## Goal

Remove BugLogin build output from the prohibited C: and F: drives, remove duplicate BugLogin build caches elsewhere, then make E: the sole clean local build target for the current BugLogin source.

## Constraints

- Stop every BugLogin API, Vite, Tauri, Cargo, and Rust process that can hold a target directory before cleanup.
- Delete only verified BugLogin build-output directories (`target` roots and dedicated Cargo `debug` roots). Preserve repositories, source, profile data, PostgreSQL data, signing material, browser artifacts, `.tmp` references, and unrelated projects.
- Do not build on C: or F:.
- Retain one canonical target on E:, currently intended as `E:\buglogin-target`; make the desktop dev launcher use that same configured root and reject C:/F: paths.
- Keep production build commands portable: the cache location is environment configuration and is never part of a shipped artifact.
- Do not work around filesystem-policy rejection. If a verified deletion is rejected, stop before building and report the exact path and blocker.

## Current evidence

- Existing target candidates were verified at `C:\buglogin-target`, `E:\buglogin-target`, `E:\BugLogin3Target`, `E:\bug-login\src-tauri\target`, `F:\BugLogin-api-gate-20261019\debug`, and `F:\Herd\bug-login\src-tauri\target`.
- The source repo's ignored `.cargo/config.toml` points to `C:\buglogin-target`; `scripts/tauri-dev-local-api.mjs` also defaults to C: and rejects only F:.
- C: had about 1.98 GiB free, E: about 87 GiB, and F: about 22.75 GiB at last check.
- A local API run briefly served the PostgreSQL-backed signed Wayfern catalog successfully, then was stopped. The desktop rebuild was interrupted at the user's request before completion.
- Recursive deletion attempts were rejected by the execution policy before any files were removed.

## Acceptance criteria

- No BugLogin `target` or dedicated build-output `debug` directory remains on C: or F:.
- Duplicate BugLogin build targets on E: are removed before the clean build.
- One documented, configured E: target is used by API and desktop Rust builds; C:/F: target paths fail before compilation.
- The clean build succeeds from current source and the desktop opens against the configured PostgreSQL local API with the signed runtime catalog available.
- The documented cleanup procedure lists exact verified paths and explicitly protects source, app data, browser runtimes, signing keys, and unrelated directories.

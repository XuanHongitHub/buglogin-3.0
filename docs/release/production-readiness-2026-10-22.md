# BugLogin production readiness snapshot — 2026-10-22

This report consolidates the local restart and read-only release, API/sync, MCP/team-lock, and macOS audits requested in this session. It records evidence, not a production-ready claim.

## Local development restart

- Closed the prior BugLogin desktop process, Vite process, and local API process; retained the PostgreSQL service and its data.
- Rebuilt and restarted the API and desktop from `F:\Herd\buglogin`, using the single build target `E:\buglogin-target`.
- Desktop process: `buglogin-desktop-app.exe`, PID 18168, start time 2026-10-22 16:22, executable `E:\buglogin-target\debug\buglogin-desktop-app.exe` (built 16:22).
- Its top-level native window is titled `BugLogin` and is visible according to Windows window enumeration.
- Vite `http://127.0.0.1:1420/` returned HTTP 200; API `http://127.0.0.1:12345/v1/readyz` returned HTTP 200 with persistence mode `postgres`; `/v1/mcp/status` returned HTTP 200.
- The frontend is served from the restarted Vite process; the desktop binary was freshly compiled from the current worktree. No profile/browser launch was performed as part of this restart.

## Release and updater

- Current BugLogin updater uses the Tauri signed updater plugin (`apps/desktop/src-tauri/src/system_features.rs:379-394`). Its fallback feed is GitHub `latest.json`; the release feed assembler emits GitHub artifact URLs (`scripts/release/assemble-tauri-updater-feed.mjs:36-45`). Signing workflow also hardcodes the GitHub feed (`.github/workflows/signing.yml:108,253`). There is no app-release S3 publishing step in the audited workflow.
- Donut's checked-in updater is a custom GitHub Releases client which downloads platform assets and validates checksums. It currently uses GitHub release assets; it does not demonstrate an S3 release host. BugLogin's approved updater design already uses Tauri signatures (`docs/superpowers/specs/2026-10-22-tauri-native-updater-design.md`).
- Minimal compatible direction for the requested BugLogin S3 hosting: keep the signed Tauri updater and its startup/manual-check UX, publish immutable signed platform artifacts and `latest.json` to the existing public Spaces desktop release path `releases/desktop/`, and update the compiled feed endpoint and artifact URLs. Keep API sync objects isolated under the `buglogin3/prod` prefix.
- Blockers: no configured `TAURI_SIGNING_PRIVATE_KEY`, matching password, or updater public-key variable was found in GitHub configuration. The legacy 1.0 Spaces bucket and desktop release prefix exist, but the 2.0 release workflow does not publish there; scoped write permission and a successful 2.0 upload/update are unverified. Current local updater cannot verify a signed update without the public key. This production path is not implemented or proven.

## API, MCP, team lock, and syncing

- Staging API verification failed on 2026-10-22. The live OpenAPI document had 94 paths / 100 operations versus current source's 110 / 122; the runtime-catalog readiness header was missing. The read-only verifier is `scripts/release/verify-staging-api.mjs`; report recorded 22 source operations absent from staging.
- Local API readiness is now PostgreSQL-backed (`x-buglogin-persistence-mode: postgres`) and `/v1/mcp/status` returns 200. This does not demonstrate authenticated MCP tool coverage; no full `tools/list` plus role-scoped CRUD/lease matrix was run in this audit.
- Local sync test evidence (`docs/e2e/local-api-auth-sync-2026-10-19.md`) proves encrypted payload handling, checksum/manifest checks, wrong-key/tamper rejection, and restart persistence with SQLite/filesystem adapters. It does not prove production-equivalent multi-client PostgreSQL/S3/Redis syncing.
- Source includes Redis-backed shared leases/presence for production and local in-memory coordination for development. No live multi-device/team-lock evidence was found; do not claim production lock correctness yet.
- The documented MCP contract is scoped to account/API-token authority and profile CRUD/runtime actions; browser-page automation, goals, and scenario execution are not in the committed contract.

## macOS and cross-platform profile portability

- The legacy 1.0 S3 manifest lists a macOS DMG, but no current 2.0 macOS build or successful 2.0 signing/notarization run was found.
- Workflow source defines Apple Silicon and Intel build/sign/notarize steps, but source configuration is not release evidence. Apple credential names exist; P12 format and readiness were not verified. Protected `release-signing` and `stable-promotion` environments were not present in the GitHub environment inventory.
- Updater signing keys/public variables are missing, and the current updater feed is GitHub rather than the requested BugLogin S3.
- Profile data layout is designed to sync logical profile files and hydrate them under per-profile local paths, but no Windows-created profile -> macOS restore/launch/write/sync-back acceptance run was found. Cookie encryption is OS-key dependent and requires explicit cross-OS validation.

## Required next evidence

1. Deploy the current API source and rerun the staging contract/runtime-catalog verifier until exact source parity passes.
2. Run the authenticated MCP tool/permission matrix and live CRUD/lease checks against the deployed API.
3. Run two-client encrypted sync, checksum, restart recovery, and team-lock verification against production-equivalent PostgreSQL, Redis, and S3.
4. Reuse the verified legacy release bucket/path only after validating scope; wire the 2.0 workflow to publish immutable artifacts and its signed update feed there. Do not provision another bucket unless the existing bucket cannot provide an isolated release prefix and restricted publisher.
5. Provision protected Tauri updater key custody and verify a signed Windows update end-to-end.
6. Run signed/notarized macOS release builds on Apple Silicon and Intel runners, then validate actual Windows-to-macOS and macOS-to-Windows profile restore/launch/sync, including cookies.

The current desktop app has been restarted locally; these production and release gates remain open.

## Follow-up gate audit (30-minute work window)

This section records the follow-up parallel audit. No production deploy, release, or data mutation was performed.

### API and MCP

- Read-only staging parity remains **94 paths / 100 operations** on `api3.buglogin.com` versus **110 / 122** in source. The runtime-catalog readiness header is absent; source operations including `auth.desktop.login` and `runtime.catalog.get` are not present on the host.
- Local `/v1/mcp/status` returns HTTP 200 and reports enabled scoped-token auth. The full authenticated role/tenant CRUD matrix was not run because no isolated scoped test credential/memberships were available. Focused Rust MCP tests did not finish: cold compilation was contending for the shared `E:\buglogin-target` lock.
- No source API defect was established. Staging deploy was not attempted: worktree is dirty (42 changed/untracked entries at audit time), and the documented deploy path requires a reviewed synchronized source revision and attested artifact. No production mutation was made.
- The current local branch is four commits ahead of `buglogin-3.0-source/main`; it is not a reviewed/synchronized release input.

### Sync and team lock

- No dedicated disposable live-test configuration was present; no local Redis/MinIO listener was available and Docker Desktop's engine was unavailable. The BugLogin 3.0 VPS does have production PostgreSQL, Redis, and DigitalOcean Spaces configured, but this audit did not run a synthetic two-client test against those services. The configured PostgreSQL URL was not used because it was not identified as a disposable test database.
- Therefore two-client S3 sync, checksum/restart recovery, and shared Redis lease/fencing are **not verified**. Storage/sync Rust tests were blocked behind the shared target lock and stopped without a result.
- A Redis integration test was added in `crates/sync/tests/coordination.rs` for separate clients, contention, release/reacquire, and monotonic fencing tokens. `rustfmt --check` and `git diff --check` pass; the test itself remains unrun until a test Redis endpoint is available.

### macOS runtime and cross-platform profile portability

- `cargo test -p buglogin-browser --test runtime_download` passed **6/6 on Windows**. The local signed runtime catalogs contain only Windows x64 ZIP entries; no Mac runtime artifact or successful Mac build/install/launch evidence exists. The GitHub platform-workspace run remains queued with no jobs.
- Cross-target compilation waited on the shared target lock and was stopped without a result. macOS-specific installation (`hdiutil`/`ditto`) has not been run on macOS.
- Windows-to-macOS profile portability remains unverified. Cookie storage paths and encryption PBKDF iterations differ by OS; no migration/re-encryption implementation or cross-OS acceptance evidence was found.
- The user-reported Donut shortcut tiles on Wayfern New Tab still lack a fresh visual acceptance run. Source has a pre-launch preference/database cleanup helper, but that alone is not evidence the bundled Wayfern page no longer renders those tiles.

### Release and updater

- `pnpm release:verify` passes **13/13** static release-policy checks.
- GitHub runs `39750308888` (Platform Workspace) and `39750347860` (Native Linux API Artifact) are still queued with no jobs. The public legacy 1.0 S3 manifest currently lists v1.1.84 Windows and macOS artifacts; no current 2.0 Windows/macOS release build is evidenced.
- GitHub account has repository `WRITE` access, not settings/admin access. GitHub secret names for S3 and Apple signing are configured. Read-only VPS inspection found the legacy Spaces bucket/path and public manifest; the 2.0 workflow still publishes only a GitHub draft prerelease and has no verified S3 write run. Updater signing custody and end-to-end release publishing remain open. No release was triggered or promoted.

### Immediate blockers to close

### Infrastructure inventory correction — 2026-10-22

The read-only inventory in [production-infrastructure-inventory-2026-10-22.md](production-infrastructure-inventory-2026-10-22.md) supersedes earlier statements that no release bucket/path exists. BugLogin 1.0 uses the public DigitalOcean Spaces bucket `mavelnut-login`, region `sgp1`, with desktop releases under `releases/desktop/v<version>/` and the feed at `releases/desktop/latest.json`. The public feed returned HTTP 200 and listed the Windows installer and macOS DMG for v1.1.84. The 3.0 API also uses that bucket with sync prefix `buglogin3/prod` and tenant `buglogin3-prod`; that is separate from the desktop release prefix. The 2.0 workflow has not yet been connected to the legacy release path, and neither successful 2.0 S3 publication nor updater acceptance is verified. No credential values were read.

1. Synchronize/review source and run the documented attested API deploy, then repeat staging API parity and authenticated MCP role tests.
2. Provide disposable BugLogin-owned PostgreSQL, S3-compatible storage, and Redis test services plus a second client; run sync/checksum/restart and shared lease/fencing tests.
3. Publish signed Mac runtime catalog artifacts and run build, install, launch, and Windows↔Mac profile/cookie acceptance on native Mac runners.
4. Provision protected updater signing keys and the approved BugLogin release host/prefix/publisher; unblock GitHub runners and produce signed Windows/macOS artifacts, then run install/update acceptance before stable promotion.

These are external authority/infrastructure and native-platform evidence gates, not claims of completed production readiness.

### Release workflow source update — 2026-10-22

The earlier audit above records the state before the follow-up S3 workflow edits. Current local source now wires BugLogin 2.0 RC artifacts and feed to the isolated Spaces namespace `releases/desktop/v2/`; stable promotion publishes a distinct stable feed. The legacy 1.0 `releases/desktop/latest.json` remains untouched. Static workflow policy and URL validation pass, and all three relevant workflows pass `actionlint`. This verifies repository source only: no release workflow was triggered, no object was uploaded, S3 write scope/public reads are unverified, and signing keys plus platform artifacts are still missing. See [the infrastructure inventory](production-infrastructure-inventory-2026-10-22.md) for the verified VPS and bucket map.

The S3 workflow edits are in the local worktree only. The committed `origin/main` workflow still creates a GitHub prerelease and has no Spaces publisher. The worktree is broadly dirty, so these edits must be isolated and reviewed before pushing. The source edits do not change the API/sync/team-lock or native macOS evidence above. Production readiness remains blocked until the listed live and platform acceptance evidence exists.

### Two-VPS live status recheck — 2026-10-22

- Legacy 1.0 VPS: both PM2 services (`buglogin-api`, `buglogin-web`) online; Nginx config test succeeds; website, API root, and `/health` return HTTP 200. Its `/v1/readyz` is 404, which is not the health route for this legacy service.
- BugIdea VPS hosting BugLogin 3.0: API, web, Cloudflare tunnel, and Redis systemd units active; API `/v1/readyz` and `app3` return HTTP 200.
- Public legacy Spaces manifest returns HTTP 200. These are availability checks only; they do not establish source/API parity, S3 write access, active cross-device sync, or release/update success. Full host mapping and non-secret route configuration are in [the infrastructure inventory](production-infrastructure-inventory-2026-10-22.md).

### Current About updater toast and release-secret check — 2026-10-22

- The screenshot's `app_updater_not_configured` was caused by a local build without the release-only Tauri updater public key. Opening About no longer triggers a check; pressing its button shows a localized informational message for this expected dev state. Network/feed/signature errors remain actionable errors. No updater background poll runs without the key; Cargo now watches updater key/endpoint variables. TypeScript typecheck, Rust format check, Windows desktop `cargo check --all-targets`, Biome lint with formatting disabled, and `git diff --check` pass. A visual click-through on a running desktop remains pending.
- Read-only GitHub secret-name inspection targeted the actual `keyduc91/Malvanut-Login` release repository. It has legacy Apple and S3 secret names, but no listed Tauri updater private-key secret or updater public-key variable. Secret values were not read. Windows Authenticode is not required by the intended release policy (users install the downloaded installer), but the current workflow still gates Windows builds on those certificate secrets and must be updated. Tauri updater signing is a separate requirement if in-app updates remain enabled. Apple signing/notarization and updater configuration still need successful workflow evidence.
- The full ordered go/no-go matrix is in [production-night-run-checklist-2026-10-22.md](production-night-run-checklist-2026-10-22.md). Current status remains NO-GO.

### Additional local verification after the audit

- With `CARGO_TARGET_DIR=E:\buglogin-target` explicitly set, API library tests passed **223/223**, focused MCP tests passed **6/6**, sync library tests passed **16/16**, storage library tests passed **24/24** (one PostgreSQL-specific test is ignored unless a dedicated test DB is configured), browser runtime-download tests passed **6/6**, and the New Tab cleanup unit test passed **1/1**. These do not replace live API role, Redis, or S3 tests.
- API `openapi_coverage` integration test did not run: Cargo could not replace `E:\buglogin-target\debug\buglogin-api.exe` while the local API process was using it. No app/API process was stopped to force that test.
- Authenticated local MCP read-only probe with the previously supplied token discovered **72 tools**. `profile_list` returned 8 profile records (6 projected `running`, 2 `ready`) while this machine had **0 Wayfern processes**. This alone cannot distinguish stale local status from a profile running on another device. The same token receives 403 for `account_get` and `organization_list`, so it cannot inspect account/workspace membership or query the workspace-scoped lease needed to identify the lock owner. No stop/status mutation was issued.
- An initial MCP test invocation inherited the shell's `CARGO_TARGET_DIR=C:\buglogin-target`; it was stopped when detected. A cleanup attempt for that target was rejected by automatic review, so the directory remains and must not be treated as cleaned. All subsequent cargo commands explicitly targeted E:.

## Profile sync payload and local transfer error follow-up

- Donut's self-hosting guide describes Donut Sync as storing profile browser data and separately synced proxies, VPNs, groups, and extensions in S3-compatible storage. The guide does not enumerate each profile file. Donut's sync docs say it syncs profile settings/browser data and excludes ephemeral profiles, in-place extensions, and 2FA keys. Its pinned client source (`.tmp/donutbrowser-upstream/src-tauri/src/sync/manifest.rs`) explicitly excludes browser caches, GPU/Dawn caches, Service Worker CacheStorage, session storage, crash/telemetry files, and other rebuildable data.
- BugLogin's active profile projection also excludes many cache directories, but was still including `BrowserMetrics*`, `DawnGraphiteCache`, `DawnWebGPUCache`, and `Service Worker/CacheStorage`. These exclusions were added to the sync projection. For the profile shown in the user's screenshot, the current generation directory measured 30.71 MiB total; the previous BugLogin filter would include about 7.53 MiB, including a 4 MiB `BrowserMetrics-spare.pma` and about 2 MiB of Dawn GPU cache. Under the updated filter, the expected projection falls to about 1.43 MiB. This size estimate applies to that local generation and is not a measurement of a completed cloud upload.
- The screenshot's exact error is a real API admission error, not proof that a profile is unusually large. Two server-wide large-transfer slots were acquired with `try_acquire_owned()`, so overlapping blob transfers immediately returned retryable HTTP 503. Stop closes the local browser before syncing the snapshot; thus the old toast could claim Stop failed even when the browser process had stopped and cloud sync was the failing step. Admission now waits up to 30 seconds for a slot, preserving the 2-transfer memory bound; client upload retries for this error use bounded exponential backoff with the existing idempotency key.
- Local launch now strips positional URL arguments from Wayfern runtime config and supplies `chrome://newtab/`, preventing a configured vendor landing URL from being passed on normal profile launch. Windows profile window/taskbar icons now use a large two-line, up-to-seven-character profile abbreviation on a BugLogin red field instead of tiny initials over the logo.
- Verification: `cargo check -p buglogin-api -p buglogin-browser -p buglogin-desktop-app` passed using `E:\buglogin-target`; local app PID 25644 is running, Vite returned HTTP 200, and local API readiness returned HTTP 200 with PostgreSQL persistence. No browser profile was launched after these changes, so the actual Wayfern new-tab content and Windows icon rendering still need a visual launch check. `cargo fmt --all --check` continues to report pre-existing formatting differences in the already-modified `crates/api/src/mcp/mod.rs`; the changed Rust sections are formatted.

## Recheck — 2026-10-23

- Source commits `160776d2`, `d0959f13`, and `fbb15411` are pushed to `XuanHongitHub/buglogin-3.0` (`main`); the only uncommitted paths are user-owned `CODEX_CHAT.md` and three migration-sidecar restore directories. Do not stage them or use `git add -A`.
- Current GitHub Actions runs for `fbb15411` start no workflow steps. GitHub annotations explicitly report an account payment failure / spending-limit hold. Making the repository public does not repair a locked billing account: [GitHub's locked-account guidance](https://docs.github.com/en/billing/how-tos/troubleshooting/locked-account) says billing locks can block Actions and other free-tier access; [public standard runners are normally free](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), but that does not override an account-level lock. Resolve the account billing hold before relying on hosted runners. The repo remains private; its earlier audit found historical credential/session-file matches, so do not expose its existing history without a dedicated scrub and verification.
- Malvanut-Login is not an executable fallback: current GitHub identity has write rather than admin rights there; its runs are also queued without jobs; its workflow publishes to the Malvanut release namespace and lacks BugLogin v3 updater configuration.
- MCP/admin source change is verified locally and pushed: `account.create` only creates a pending, non-login-capable account; it does not grant platform admin. Actual API key execution tests cover account provisioning, organization/workspace CRUD, member-role change, groups, profiles, and WorkspaceAdmin scope boundaries. The fix aligns wildcard profile-create authorization with hosted provisioning checks. `cargo test --target x86_64-pc-windows-msvc -p buglogin-api --test mcp_execution` passed 33/33; contract checker passed 136 commands. Production remains on its older API and does not expose these changes.
- The 3.0 API systemd unit is active, but `/opt/buglogin3/buglogin-admin-password-reset` is absent on the host. The supported operator password-reset path is therefore not currently available there; no production admin password or role was changed.
- Cross-OS sync audit confirms ciphertext is platform-neutral but key material stays in each OS's native vault. Cross-device portability requires explicit recovery-code import or a device-targeted key envelope; same-account login alone does not prove key availability. Manual recovery now validates the active manifest key version and replaces a stale local key only when the imported key matches it. New Windows tests pass for two independent clients hydrating the same profile into separate roots with matching content/root checksums, and for stale-key replacement. This is a synthetic client/vault round trip, not actual Windows-to-macOS native-vault acceptance; macOS Keychain execution remains unverified.
- Browser version test coverage now includes first-time ZIP install from an empty runtime root followed by installing a second signed-catalog version while preserving both version-specific binaries. The focused Windows `runtime_download` suite passes 8/8. Existing desktop launch-target coverage asserts version switching preserves the profile data path, but that desktop test was not rerun because it waited on the shared G target build lock. No real Wayfern package download or launch was performed in this packet.
- Current Windows release candidate in `G:\buglogin-target` runs locally and has installer/executable hashes, but it lacks release attestation and must not be published. No macOS artifact, signing/notarization proof, S3 upload, production API rollout, signed update, or production platform-admin login is verified.
- Runtime catalog remains unavailable in production: `/v1/readyz` returns 200 without its signed-catalog readiness header and `/v1/runtime/catalog` returns 404. Production OpenAPI remains 94 paths / 100 operations and fails source parity. The RC/stable feed keys return 403 and have not been published or verified. A disposable object test using the release workflow's `public-read` ACL successfully PUT, authenticated GET, public GET, checksum comparison, and DELETE under `releases/desktop/v3/probe/`; no credential was emitted and the test object was removed.

### Current release exit blockers

1. Resolve GitHub account billing lock (or provide an authorized, working release runner). Public visibility alone is not a safe or sufficient workaround.
2. Deploy the reviewed source through the documented attested API path; verify OpenAPI parity, signed runtime-catalog readiness, MCP permissions, and S3 access.
3. Complete the independent-vault cross-OS recovery test and real Windows/macOS profile launch/sync acceptance.
4. Produce release-attested Windows and signed/notarized macOS artifacts, publish to the BugLogin S3 v3 namespace, and prove install/update including restart and preserved profile state.
5. Provision and verify the requested production platform-admin login through the supported operator/bootstrap flow; a pending account is not an admin login.

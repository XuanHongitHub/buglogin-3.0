# Local API auth and sync E2E — 2026-10-19

## Scope

Exercise the local Rust API with durable local adapters and the desktop REST/auth configuration without using BugLogin 1.0 or production data. This is implementation evidence for the local API path; it is not release acceptance.

## Evidence

| Flow | Evidence | Result |
|---|---|---|
| API process auth, CRUD, managed binding, and restart | `$env:BUGLOGIN_E2E_SKIP_BUILD='1'; $env:CARGO_TARGET_DIR='C:\buglogin-target'; node tests/e2e/support/local-api-process-smoke.mjs` | PASS (2026-10-19): prebuilt API binary runs against an isolated temporary SQLite/filesystem store; wrong-password login returns generic `invalid_credentials` without issuing tokens; valid login reads the workspace, creates/lists/updates/deletes a proxy, creates/lists/reads/deletes a profile, verifies managed resource binding and initial manifest, then restarts the API and confirms workspace/proxy/profile/binding/manifest persistence. After restart, refresh rotates both tokens with no-store cache policy; replaying the old refresh token returns `session_revoked` and revokes the replacement access token. |
| Direct login and workspace switch/data isolation | `$env:BUGLOGIN_E2E_SKIP_BUILD='1'; $env:CARGO_TARGET_DIR='C:\buglogin-target'; node tests/e2e/support/local-api-workspace-switch-process.mjs` | PASS: real local API process, direct desktop credential login, account/organization/workspace listing, create second workspace, switch request scope A↔B, profile/proxy CRUD in B, profile-to-proxy binding and isolation from A. This proves API workspace scoping by workspace ID, not desktop remembered-workspace UI, multi-organization, multi-member role isolation, or a server-side active-workspace session. |
| Desktop remembered workspace persistence | `$env:CARGO_TARGET_DIR='C:\buglogin-target'; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib desktop_state::tests::workspace_selection_persists_across_restart_and_revalidates_authority -- --exact --test-threads=1` | PASS (2026-10-19): last selected workspace persists to disk independently per account, survives store reload, and is replaced with an authorized workspace if the remembered ID is no longer in the account's current authorized set. This tests Rust persistence/authorization selection logic, not native UI rendering. |
| Desktop REST bridge action routing | `$env:CARGO_TARGET_DIR='C:\buglogin-target'; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml profile_actions_bridge_routes_group_proxy_and_trash_mutations --lib` | PASS: mocked HTTP bridge confirms command IDs, methods, paths, request bodies, bearer and idempotency headers for group replacement, proxy edit/reassignment, archive and restore. This is Rust bridge routing evidence, not native UI evidence. |
| Encrypted profile + proxy sync through API process | `$env:BUGLOGIN_E2E_SKIP_BUILD='1'; $env:CARGO_TARGET_DIR='C:\buglogin-target'; node tests/e2e/support/local-api-encrypted-sync-process.mjs` | PASS (2026-10-19): creates a credentialed workspace proxy and assigned profile; device 2 sees the same profile→proxy assignment, proxy revision, and opaque credential version; reads/decrypts the AES-GCM profile payload and the separately scoped proxy-credential envelope, verifies checksum/content, and confirms proxy secrets are absent from API metadata; wrong key and modified authentication tag are rejected; after API restart a third login confirms encrypted blob persistence. Uses isolated SQLite and filesystem chunks, not production services. |
| Sync coordination routes | `cargo test -p buglogin-api --test local_coordination_e2e` | PASS: SQLite control store, filesystem chunk store, and real `LocalCoordination`; readiness, HTTP lease acquisition, fenced manifest commit, and manifest readback. |
| Signed control + sync package activation | `cargo test --locked -j 1 -p buglogin-api --test http_signed_sync_package -- --test-threads=1` | PASS (2026-10-19): source-current test covers control rollback when sync lease acquisition fails, retry without an empty genesis-manifest conflict, serialization of concurrent activation, activation replay, and authoritative manifest readback. The focused build temporarily disabled autobins to avoid replacing the running API executable; that Cargo setting was removed immediately after the test. |
| Auth, workspace, CRUD, encrypted sync, and reopen | `cargo test --locked -p buglogin-api --lib http::auth_workspace_profile_e2e::desktop_login_workspace_list_and_profile_crud_use_isolated_api_state -- --exact --test-threads=1` | PASS: synthetic desktop login and workspace; legacy profile creation with `groupIds` is rejected with `profile_groups_mutation_required` and a GET confirms no profile was persisted; normal profile/proxy operations and encrypted sync across devices continue; persisted control, manifest, and chunk data survives reopening the local stores. Coordination in this test is a test adapter, covered separately above by the real-local-coordination route test. |
| API crate regression suite | `$env:CARGO_TARGET_DIR='C:\buglogin-e2e-source-current'; cargo test -j 1 -p buglogin-api -- --test-threads=1` | PASS (2026-10-19): 342 tests across the library, binaries, REST/MCP HTTP integration, migration, sync, and readiness targets. Serial execution avoids Argon2 saturation between auth tests. |
| Pending identity login rejection | `cargo test -p buglogin-api --lib web_and_desktop_login_reject_pending_identity_generically` | PASS: hosted web and desktop login both return the same generic invalid-credentials response and issue no session for a pending identity. |
| Local coordination behavior | `cargo test -p buglogin-sync local_backend --lib` | PASS: 6 tests for lease fencing/idempotency/expiry, ownership, presence TTL, event order, and manifest generation. |
| Contract parity | `pnpm check:api` and `pnpm test:contract` | PASS: generated API registry and canonical REST/OpenAPI/Tauri/MCP contracts match; 35 contract checks pass. |
| Desktop frontend checks | `pnpm --filter @buglogin/desktop test` and `pnpm --filter @buglogin/desktop typecheck` | PASS. |
| Browser rendering of desktop login route | Playwright navigated to `http://127.0.0.1:1420/` | PASS for rendering only: BugLogin sign-in form and branding render, console has no errors; controls are disabled with “Desktop service unavailable” because a normal browser has no Tauri native bridge. This is not native desktop login E2E. |
| Isolated native login attempt | Bootstrapped a synthetic local owner and started source-current API on `127.0.0.1:12345` with a separate temporary data root | API bootstrap/readiness passed. Automatic review blocked starting a second Tauri desktop instance, so native login was not attempted. The test API was stopped and port 12345 is closed; the existing desktop/API processes were left running. A 72 KB synthetic test data root remains under the machine TEMP directory because recursive cleanup was also blocked by automatic review. |

The local API now uses hosted account/profile/sync authority with durable SQLite control storage and a local filesystem chunk adapter. Local accounts are provisioned by the explicit development bootstrap command; the API itself no longer silently provisions local identities. Build artifacts and Cargo target data were directed to `C:\buglogin-target`. The interactive local API data root is `C:\BugLogin-Local-Dev-20261019`; it contains only a synthetic local owner and workspace.

## Public 3.0 API comparison (read-only)

On 2026-10-19, `https://api3.buglogin.com/v1/readyz` returned 200. Its OpenAPI document returned 200 with 94 paths / 119 operations and SHA-256 `5763cf4f5ae35ff892a254a8c87d7936180d64d1d24a1aef4573a7cca21b5673`. The worktree contract has 96 paths / 121 operations and SHA-256 `b0cfdb6a3db55fc75afac1becf367ed23b56ddca3e45ecc8ddd1f3e714af77a6`. There were no live-only operations; the local-only operations are `POST /v1/auth/desktop/login` and `GET /v1/runtime/catalog`. The live readiness response had no `X-BugLogin-Runtime-Catalog` header, and a GET to that catalog path returned 404. No authenticated request, mutation, or deployment was made.

## Not proven

- The desktop's local API process on `127.0.0.1:12344` was launched at 01:47 from `C:\buglogin-target\debug\buglogin-api.exe`, before the current source edits. A source-current API executable was built at 05:41 as `C:\buglogin-target\debug\buglogin-api-source-current.exe`, but was not switched into the running desktop because the process restart was blocked by the automatic review gate. The live OpenAPI bytes match the worktree SHA-256, but that does not prove the running backend implementation is source-current.
- Native Tauri UI automation from login through workspace, profile/proxy CRUD, and sync.
- The Playwright-connected page at `http://127.0.0.1:1420/` is a plain Chrome page without the Tauri bridge; its desktop fields are disabled and it reports `Desktop service unavailable`. It cannot stand in for native Tauri E2E.
- A read-only capture of the running native Tauri window visually confirmed the current email/password login form, BugLogin asset, and Donut-style two-column composition. Windows UI Automation exposes only a generic `BugLogin - Web content` pane, the WebView2 process has no remote-debugging endpoint, and no Tauri/WebView2 driver is configured. Native login/workspace/profile/proxy interaction automation was therefore not run; visual inspection is not an interactive E2E.
- Browser profile runtime download, verification, installation, or launch. A local runtime catalog/artifact must be configured and trusted.
- Native desktop integration of login/workspace/profile screens with the encrypted sync flow. The API process E2E covers authentication, multi-device encrypted sync, and restart persistence independently of Tauri UI automation.
- Runtime launch through a signed BugLogin runtime catalog. The local route currently returns `503 runtime_catalog_unavailable`; a 2.1 GB Donut Wayfern install exists on this workstation but is not a signed BugLogin release artifact and was not used.
- Hosted PostgreSQL/S3/Redis behavior, cross-process/multi-device sync, production OpenAPI parity, macOS behavior, or production release readiness.
- Full transfer interruption/process-death recovery, shared-profile key revocation/rekey, and visual/accessibility acceptance.

Do not interpret local SQLite/file-store success as hosted production acceptance.

## Consolidated local E2E pass — 2026-10-19

The following process-level E2Es ran with `BUGLOGIN_E2E_SKIP_BUILD=1`, `CARGO_TARGET_DIR=C:\buglogin-target`, isolated temporary data roots, and ephemeral loopback ports. They did not stop or replace the desktop's API process:

| Flow | Result |
|---|---|
| Credential rejection/login, profile and proxy CRUD, refresh rotation/replay revocation, API restart persistence | PASS |
| Organization/workspace listing, create workspace B, A↔B request scoping, profile/proxy isolation | PASS |
| Cross-device encrypted profile/proxy sync, AES-GCM key/tag rejection, manifest/checksum, restart persistence | PASS |

Prebuilt source-test executables also passed auth (15/15), proxy (4/4), groups (3/3), sync migration (2/2), and the integrated auth/workspace/profile/encrypted-sync/reopen case (1/1). The profile HTTP suite was 8/9: its remaining test expected profile metadata mutation to succeed while a proxy assignment is pending, but the current API guard intentionally returns `profile_proxy_assignment_pending`. The test source now asserts that guard and restages against the unchanged profile revision. `cargo check --locked -j1 -p buglogin-api --tests` passes with the updated test source. Executing the rebuilt test remains blocked because Cargo could not replace the running `C:\buglogin-target\debug\buglogin-api.exe` (Windows access denied); the modified case is type-checked but not yet executed.

The native desktop remained on screen with an empty Donut-style email/password login form. No credentials were submitted because no safe local test credential was available, so native login and workspace switching remain unverified. The live local API readiness endpoint returned 200 with SQLite persistence, while the signed runtime catalog remained unavailable (503); browser installation/launch and mobile runtimes remain unverified. The API process is still the binary started at 01:47, before current source edits. Do not treat these results as hosted PostgreSQL/S3/Redis or release evidence.

A direct POST to the running local `/v1/auth/desktop/login` with a unique, nonexistent synthetic email returned `401 invalid_credentials` and no token fields, confirming the route is present on that process. This is only negative-credential behavior; it does not verify successful native login.

## E2E continuation — 2026-10-19

Reran all three isolated API process journeys against the source-current API executable copied to `C:\buglogin-e2e-source-current` (leaving the desktop's running API untouched); each exited 0: credential rejection/login + profile/proxy CRUD + restart/refresh replay; workspace A↔B isolation; and encrypted cross-device sync + wrong-key/tag rejection + restart persistence. `pnpm --filter @buglogin/desktop test`, desktop typecheck, `pnpm check:api`, and `pnpm test:contract` also pass (35 contract tests; 59 capabilities, 50 work packets, 114 commands).

Source-current Rust API library verification: `CARGO_TARGET_DIR=C:\buglogin-target cargo test --locked -j1 -p buglogin-api --lib -- --test-threads=1` passed **212/212** in 59.97s. Coverage includes auth/session lifecycle, account-status fencing, workspace/profile/group authorization, profile sharing and key envelopes, sync manifests/import recovery, lease fencing, quota, and runtime catalog validation. Compiler emitted existing `unused_mut`/`dead_code` warnings; no failures. This library run did not replace the running `buglogin-api.exe` and is not hosted PostgreSQL/S3 evidence.

The active Vite page at `http://127.0.0.1:1420/` was inspected with Playwright. CSS is loaded and the login form renders with the Donut shared components/tokens; the page correctly reports `Desktop service unavailable` because a plain browser has no Tauri bridge. It is not valid evidence for native login. Source tracing confirms `AuthBootstrap` submits email/password through the registered Tauri `desktop_auth_login` command to `POST /v1/auth/desktop/login`, then mounts Donut `DesktopHome`; workspace selection is persisted per account and profile listing uses the active workspace. Native credential submission and post-login UI interaction remain unverified because no safe interactive Tauri driver/test account is available.

The running local API readiness endpoint is 200 with SQLite persistence. A request to `/v1/runtime/catalog` with the required correlation ID returns `503 runtime_catalog_unavailable`; no signed local runtime catalog/artifact is configured, so browser install/launch cannot pass in the active app. This local limitation is separate from the public API contract drift recorded above. No existing app/API process was stopped or replaced.

## Isolated API process rerun — 2026-10-19

Reran the three process E2E scripts with `BUGLOGIN_E2E_SKIP_BUILD=1` and `CARGO_TARGET_DIR=C:\buglogin-target`. Each script provisions its own synthetic local owner, uses a temporary data root and an ephemeral loopback port, and stops only the API child process it started. All three passed:

| Command | Result |
|---|---|
| `node tests/e2e/support/local-api-process-smoke.mjs` | PASS: invalid credential rejection, direct login, profile/proxy CRUD, API restart persistence, refresh rotation and replay revocation. |
| `node tests/e2e/support/local-api-workspace-switch-process.mjs` | PASS: direct login, organization/workspace listing, second workspace creation, A↔B request scoping, profile/proxy CRUD and cross-workspace isolation. |
| `node tests/e2e/support/local-api-encrypted-sync-process.mjs` | PASS: encrypted profile/proxy sync across devices, AES-GCM wrong-key/tag rejection, manifest/checksum, and API restart persistence. |

These are isolated API process checks against the prebuilt binary. They do not stop or replace the desktop's existing API process, and do not cover native UI interaction, browser runtime launch, hosted PostgreSQL/S3/Redis, or public staging parity.

## Public staging contract recheck — 2026-10-19

`pnpm release:verify:staging-api` performed read-only GETs against `https://api3.buglogin.com`. It failed as expected: readiness says `ready` but has no `X-BugLogin-Runtime-Catalog` header; live OpenAPI is 94 paths / 100 operations, SHA-256 `5763CF4F5AE35FF892A254A8C87D7936180D64D1D24A1AEF4573A7CCA21B5673`, versus the current worktree's 96 paths / 102 operations, SHA-256 `B0CFDB6A3DB55FC75AFAC1BECF367ED23B56DDCA3E45ECC8DDD1F3E714AF77A6`. The live API is missing `auth.desktop.login` and `runtime.catalog.get`; it has no unexpected operations. This confirms the deployed API cannot yet serve the current direct desktop-login contract. No authenticated request, mutation, or deployment was made. This recheck supersedes earlier operation counts and hashes in the public-comparison section above.

Latest source-current recheck (2026-10-19): `pnpm release:verify:staging-api` still returns nonzero. Staging reports ready but omits `X-BugLogin-Runtime-Catalog`; live OpenAPI remains 94 paths / 100 operations (`5763CF4F5AE35FF892A254A8C87D7936180D64D1D24A1AEF4573A7CCA21B5673`) while the worktree is 97 paths / 103 operations (`CEB9BA450155045A6ECEAD25036EA42BAA93B93E5B3166B0427C28DE221E211F`). Live staging lacks `auth.desktop.login`, `grant.list.legacy`, `grant.list.page`, and `runtime.catalog.get`, and still exposes the old `grant.list` operation. This is a read-only contract check; no authenticated request, mutation, deployment, or DNS change was made.

## Source-current E2E rerun and share UI — 2026-10-19

Reran all three isolated API process E2Es with `BUGLOGIN_E2E_SKIP_BUILD=1` and `CARGO_TARGET_DIR=C:\buglogin-e2e-source-current`, whose API binary hash matches the source-current executable. All passed: auth/profile/proxy CRUD and refresh replay revocation; workspace A↔B isolation; encrypted profile/proxy sync, key/tag rejection, checksum, and restart persistence. Each run used its own temporary SQLite/filesystem data root and loopback port. The existing desktop/API processes were not stopped or replaced.

The Donut desktop has a Tauri-backed profile-share dialog with per-profile and bulk entry points, read/operate grants, revoke actions, recipient-device fingerprint confirmation for encrypted profiles, and per-profile result reporting. Native dialog interaction and screenshot QA remain unverified because the Playwright page is a browser without the Tauri desktop bridge. Cursor support is documented in the later grant pagination section.

## Cursor grant sharing E2E — 2026-10-19

Implemented bounded grant pages (`GrantId` order, limit 1–100, resource/member filters, existing sharing authorization) at `/v1/workspaces/{workspaceId}/grants/page`. The unpaginated array endpoint remains for REST compatibility and is now identified as `grant.list.legacy`; the new desktop command is `grant.list.page`. Desktop member and grant collection readers follow cursors, detect cycles, and enforce a page ceiling.

Verification against the source-current API binary in `C:\buglogin-e2e-source-current`:

- `cargo test -p buglogin-api grant_pages_are_bounded_filtered_and_continue_by_id`: PASS.
- `node tests/e2e/support/local-api-process-smoke.mjs`: PASS; process E2E now checks the authenticated grant page response shape and limit.
- `node tests/e2e/support/local-api-workspace-switch-process.mjs`: PASS.
- `node tests/e2e/support/local-api-encrypted-sync-process.mjs`: PASS.
- `pnpm --filter @buglogin/desktop test`, `pnpm --filter @buglogin/desktop typecheck`, `node scripts/generate-api.mjs --check`, and `node scripts/ci/check-contracts.mjs`: PASS.

This does not prove native Tauri profile-share interaction. Invalid page limit, denied sharing authorization, and explicit legacy-array compatibility are covered by `control_http_flow_enforces_acl_replay_audit_and_redaction`; the test asserts a 400 `invalid_page_limit`, 403 ACL denial, and the old filtered route still returns an array. MCP execution test `production_mcp_grant_list_page_continues_and_legacy_list_keeps_all_items` passed, covering ordered cursor continuation and legacy all-items behavior.

The running local API remains an older binary than the current source and does not expose `/grants/page`. Its runtime catalog is unavailable (`503 runtime_catalog_unavailable`), so browser install/launch cannot complete through the currently open app. Do not treat the visible Tauri window/HMR or isolated API process tests as a native login-to-profile browser E2E. Native credential submission and interactive profile share/launch remain unverified; no credentials were entered and no running app/API process or data was replaced.

## Resumed local desktop/API check — 2026-10-19

The source-current API process is live on `127.0.0.1:12345` from `C:\buglogin-e2e-source-current\debug\buglogin-api.exe`, with isolated SQLite data at `C:\buglogin-local-current-20261019-02`; `/v1/readyz` returns `ready`. The older `C:\buglogin-target\debug\buglogin-api.exe` remains independently live on port 12344. A responsive Tauri dev window (`C:\buglogin-target\debug\buglogin-desktop-app.exe`) and Vite on port 1420 are also live; the dev launcher was configured with API origin 12345 and external target/cache roots on C:.

Read-only user-flow check against API 12345 (except normal session creation from login): the synthetic `local-owner@buglogin.test` account authenticated with direct desktop login, `/v1/account` returned its account ID, `/v1/organizations` returned one organization, and `/v1/organizations/{organizationId}/workspaces` returned `Main Workspace`. The local first-owner bootstrap implementation assigns `Role::PlatformAdmin`. An initial request without `x-correlation-id` correctly returned 400; the authenticated rerun used the required header. No profile/proxy writes were made. This verifies the live local API auth and workspace-discovery path, not credential submission in the Tauri UI, UI rendering after login, or production admin access. No token was retained in evidence.

The public staging API contract remains stale as documented above. No production login, mutation, deploy, or DNS change was attempted.

## Latest local verification — 2026-10-19

Against the current worktree, `cargo test --locked -j1 -p buglogin-api --lib -- --test-threads=1` passes **213/213**. This includes direct desktop-login/workspace/profile flow, account/session and refresh revocation, authorization and profile/proxy assignment guards, sync import recovery, encrypted manifest/key handling, and runtime-catalog validation. `cargo check --offline` for `apps/desktop/src-tauri` also passes; existing warnings remain in unrelated modules. Desktop TypeScript typecheck and Biome lint for the touched terms/dialog files pass.

The Wayfern consent flow now checks BugLogin's installed-runtime command before prompting, passes the required correlation ID when accepting, and uses the BugLogin runtime executable to persist consent. Dialog chrome now queries the native decoration setting before adding Donut-style custom window controls. Vite remains available on port 1420 for frontend HMR; the currently running Tauri binary predates the Rust command additions, so those commands require the next app restart. Native visual/interaction verification was not performed in this pass.

Production remains blocked by verified live contract drift: `api3.buglogin.com` lacks desktop login, paginated grants, and runtime catalog operations present in source, and the runtime catalog is unavailable locally and on staging. Consequently, verified browser runtime install/launch and hosted multi-device PostgreSQL/S3 sync are not established by the local 213-test result.

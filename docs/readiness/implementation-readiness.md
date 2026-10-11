# Implementation Readiness Checklist

## Gate 0

- [x] Product shape and persona fixed.
- [x] Windows/macOS launch matrix fixed.
- [x] Rust/TypeScript architecture fixed.
- [x] Runtime policy fixed.
- [x] Sync/control topology fixed.
- [x] MCP/REST/Tauri authority fixed.
- [x] Migration and rollback principles fixed.
- [x] AGPL development baseline fixed.

## Artifacts

- [x] Architecture ADRs complete.
- [x] Versioned bootstrap domain, command, error, event, runtime, and adapter contracts complete.
- [x] Capability registry complete: 59 capabilities.
- [x] Agent ownership manifest complete: 50 disjoint work packets.
- [x] Design system, IA, and state matrix complete.
- [ ] Approved production reference package for every P0 screen complete under `docs/design/ui-authority.md`.
- [x] Migration package, fixture catalog, rehearsal, and rollback runbook complete.
- [x] E2E acceptance pack complete.
- [x] Build/signing/release matrix complete.
- [x] Upstream code baseline present and hashed.
- [x] Rust workspace scaffold compiles.
- [x] Toolchain pins recorded and checked in.

## Controlled implementation gate

**Status: GO for controlled implementation waves; NOT production-release ready. Last reviewed: 2026-09-02.**

Allowed now:

- Contract-backed desktop, web, control, sync, migration, REST, MCP, and E2E implementation.
- Isolated non-production infrastructure and sanitized migration rehearsals.
- P0 UI implementation only with the binding UI authority, real/sanitized data, approved reference package, screenshot QA, interaction QA, and accessibility states.

Still blocked:

- Production sync cutover until PostgreSQL and S3 adapters pass against trusted TLS staging endpoints.
- Production migration cutover until production-clone rehearsal, write-freeze, verification, and rollback evidence pass.
- Platform release until Windows, macOS arm64, and macOS Intel/waiver evidence exists.
- Distribution until signing/notarization secrets and license review exist.
- Production UI approval until every P0 screen has a Donut-compatible reference package and passing visual/interaction evidence.

### Current P0 blockers

- Cookie authority now has durable metadata/generation/idempotency receipts, restart and multi-process lock tests, staged-import invalidation, and local Tauri coverage. Local REST/MCP expose metadata-only `cookie.list`; cookie mutations fail closed until connected-device transport exists. Hosted REST/MCP connected-device job transport, native Wayfern commit evidence, and native vault evidence remain missing.
- Profile clone/share/revoke now advertise and invoke through Tauri; desktop proxy inventory now supports revision-safe create/edit/delete plus quick parse. Full desktop breadth, native interaction evidence, and Donut visual equivalence remain missing.
- Migration synthetic rehearsal and rollback-retry tests pass; production-clone export, write-freeze, two rehearsals, and production rollback evidence remain missing.
- Native macOS arm64/Intel, trusted-TLS adapter, and signed/notarized release evidence remain missing.
- Windows native shell source/config now includes Donut-compatible tray and deep-link wiring; runtime interaction evidence remains pending.
- Redis idempotent lease replay now reclaims expired/lost-response records atomically without deleting a competing live lease; live Redis expiry evidence remains environment-dependent.

## Infrastructure before platform-sensitive implementation

- [x] Windows test runner available.
- [ ] macOS Apple Silicon runner available.
- [ ] macOS Intel runner or explicit support waiver available.
- [x] Non-production PostgreSQL available.
- [x] Non-production Redis available.
- [x] Non-production S3/MinIO namespace available.
- [ ] Signing/notarization secrets configured outside the repository.
- [x] Sanitized migration fixtures available.

## Current implementation evidence — 2026-07-21

- Windows Rust workspace, API/control, Tauri migration, desktop typecheck/tests, web typecheck/build, migration rehearsal, contract, secret-boundary, and release static gates pass.
- Isolated services run from `docker/e2e/docker-compose.yml`; strict connectivity evidence is recorded at `tests/live-infra/artifacts/live-infra.json` with PostgreSQL `SELECT 1`, Redis `PING/PONG`, and MinIO HTTP health all passing.
- Redis coordination adapter live test passes against the isolated Redis service.
- Control state, revision, audit response, and idempotency replay persist across store reopen; differing request fingerprints fail with `idempotency_conflict` before domain mutation.
- Session hashes, expiry, revocation, atomic bearer refresh, logout, session/device inventory, and owner-scoped revoke now persist through the shared control store; API restart, rotation, collision rollback, HTTP refresh, session-management, and post-restart revocation tests pass, and raw bearer tokens are never persisted. Argon2id credential login, browser Secure HttpOnly cookie + session-bound CSRF, same-origin enforcement, browser session projection, web device approval, and Account Security UI are implemented with targeted tests. Recovery and scoped API tokens are implemented across Rust persistence, REST contracts, and web UI with hash-only storage, rate limiting, one-time consumption/display, owner-scoped revoke, expiry, audit, session revocation, and domain-level capability/action scope enforcement. Production startup now requires an HTTPS recovery webhook plus Rust-only secret; live hosted delivery/auth evidence and browser screenshot QA remain P0 blockers.
- Local full-stack smoke evidence is recorded at `docs/e2e/web-auth-local-stack-2026-07-23.md`: Rust API, Next web host, same-origin `/v1` proxy, `/login`, `/device`, health, and cleanup pass without Docker. This is not production-release evidence.
- PostgreSQL and S3 production adapters still require a trusted TLS staging endpoint; local plaintext fixture health does not prove those adapters.
- Trusted-TLS adapter probes are available via `cargo test -p buglogin-storage --features integration-tests --test live_production_adapters`; PostgreSQL evidence requires `sslmode=verify-full`; release CI must set `BUGLOGIN_LIVE_ADAPTER_STRICT=1` plus the test endpoint variables so missing infrastructure fails closed. The S3 probe writes a uniquely named small canary under the configured test prefix.
- macOS native runtime, signed Windows release, macOS signing/notarization, real Wayfern artifact launch, and immutable Git provenance remain release blockers.
- The native BugLogin 2.0 Tauri bootstrap injects `DesktopState::Local` or `DesktopState::Production`, exposes typed command wrappers, uses isolated platform data roots, and fails closed when the signed/checksummed Wayfern runtime is not configured. Missing runtime/deployment configuration now opens an explicit unavailable state instead of crashing the process; Windows release executable smoke stayed alive for 8 seconds, with authority disabled. `apps/desktop/src/main.tsx` is the only native desktop UI entrypoint and mounts the Donut-pattern operational shell in `apps/desktop/src/ProfileWorkspace.tsx`; the Next host is web-only. Desktop model/bridge tests, typecheck, Vite build, and the 1440x900 empty/offline screenshot evidence pass. Native Windows/macOS interaction evidence remains pending.
- `pnpm release:verify` now fail-closes version parity across package, Cargo, Tauri, and macOS bundle identity. This is static policy evidence only.
- The migration system now implements read-only inspect/snapshot, redacted or OS-vault encrypted export, package verification, non-mutating collision/idempotency dry-run, confirmed staging/promotion, and rollback-on-activation-failure. Proxy credentials, VPN configs, and profile/cookie payload indexes now use schema v2 and contain no plaintext checksum/fingerprint fields or plaintext-derived object keys; AEAD, ciphertext hashes, exact object sets, and length/binding checks provide integrity. Package HMAC-SHA256 signatures use a separate vault-backed signing key and are verified before REST/Tauri/CLI staging. Full control-plane packages use the authenticated API migration flow, which activates local profiles and control-plane ACL/share graph against the same profile repository root; standalone CLI import rejects such packages rather than silently dropping graph state. Proxy credentials and VPN configs are authenticated encrypted package objects imported into native OS vault namespaces, reconciled by vault version/hash, and removed by CAS rollback only when journal-owned. Extension artifacts are checksum-bound and rollback removes only journal-owned files. Legacy VPN AES-256-GCM storage is decrypted fail-closed, validated, immediately re-encrypted for the migration package, and never placed in entity JSON or logs. Interrupted full-graph resume, 70-case interactive E2E coverage, and real RC promotion/canary lanes remain unimplemented.
- Browser lifecycle now retains process ownership and the profile filesystem lock when stop fails; concurrent lifecycle managers cannot launch the same directory. Domain and browser suites pass without Docker.
- Migration now preserves the canonical `ProfileResource(profileId, resourceId, workspaceId)` mapping in `entities/profile-resources.jsonl`; control import validates the complete graph and persists/replays/rolls back bindings atomically. Full migration and API suites pass without Docker.
- Sync manifest retry now re-evaluates a prior generation-gap conflict after the missing parent arrives; the full in-memory sync suite passes without Docker. Workspace scope, tombstones, reconnect-gap resnapshot, and end-to-end fencing remain release blockers.
- Redis coordination leases now allocate a persistent workspace/resource fencing counter, preserve the fence across renew/release, reject corrupt stored leases, and expose only redacted lease diagnostics. The full no-Docker sync suite passes; durable Redis failover policy and desktop enforcement remain release blockers.
- Desktop now routes profile launch/stop through an async single-owner runtime actor with canonical binding resolution, local offline launch, managed acquire-before-process fail-closed behavior, `Ready → Launching → Running` and `Running → Stopping → Ready` persistence, renewal handling, stop-before-release ordering, cleanup-pending retries, shutdown cleanup, and Rust-internal write authority. The full desktop Rust suite passes; production Tauri startup injection and live managed coordination evidence remain incomplete.
- Command registry metadata now owns the canonical ACL action and generated TypeScript metadata; command-registry, compatibility, contract, and generation drift gates pass. MCP policy/executor/transport, including archive/restore, is implemented and covered by targeted tests; live multi-user transport evidence remains pending.
## Current implementation evidence — 2026-07-22

- Desktop deployment mode is now mandatory: `local` owns a standalone repository; `production` rejects `BUGLOGIN_DATA_ROOT`, requires remote HTTPS `BUGLOGIN_API_BASE_URL` plus Rust-only `BUGLOGIN_API_TOKEN`, and constructs `DesktopState::Production` without opening `AdapterState` or a local metadata repository. Hosted health, profile list/create, and operation reads carry bearer authentication, correlation IDs, and idempotency keys. Runtime launch/stop and proxy persistence still fail closed until their sync/lease contracts are complete; production never falls back to local/self-hosted authority. Desktop transport tests, 29 desktop Rust tests, API unit tests, frontend typechecks, and Rust formatting pass.


- Migration cutover now resumes a completed `.pending` journal after a crash before directory promotion; staged package identity, journal checksum, artifact hashes, and sizes are revalidated before rename. Incomplete or mismatched pending state remains fail-closed. `cargo test -p buglogin-migration --tests` passes.
- API launcher now selects durable PostgreSQL control persistence when `BUGLOGIN_POSTGRES_URL` is configured and refuses `BUGLOGIN_ENV=production` without it. API unit/HTTP profile, control, import, and migration tests pass; the missing-production-config path exits non-zero.
- Desktop package build wiring targets `apps/desktop/dist`; `apps/desktop/src/main.tsx` mounts the Donut-pattern `ProfileWorkspace` shell with real profile list/create/launch/stop, proxy parse, offline/error, job, and selection states. Full command parity, populated-state visual approval, and native Windows/macOS interaction evidence remain incomplete.
- The Donut shell core profile flow now crosses a Rust-owned compatibility boundary for `list_browser_profiles`, `create_browser_profile_new`, `launch_browser_profile`, `kill_browser_profile`, and `check_browser_status`. Launch/stop wait for canonical operations and emit state events only after authority confirms completion; canonical lifecycle state replaces fake process IDs. Group/proxy/VPN/settings and remaining legacy desktop command parity are still incomplete and block full desktop E2E.
- Proxy text parsing now crosses the Rust network parser through `parse_txt_proxies`, supports URL, host-first, credential-first, and `socks://` forms, and returns explicit parsed/invalid rows. Parsed/imported proxies persist through the versioned local repository; credentials go to the OS vault and never enter entity JSON or list responses. Donut JSON exports accept both `type` and legacy `proxy_type`, preserve proxy names, deduplicate endpoints, and emit `proxies-changed`. Desktop and web proxy CRUD use revision-safe existing contracts. Proxy check, profile reassignment, hosted production proxy sync, and remaining proxy UI commands remain incomplete.
- Hosted launch-sync contract draft now exists at `docs/contracts/production-launch-sync-v1.yaml`: profile-resource binding, Redis-backed lease fencing, manifest commits, and reconnect semantics. Redis lease acquire currently lacks idempotent replay in the shared backend trait, so no HTTP lease handler or production launch path is enabled until a lost-response retry cannot strand an opaque lease token.
## Non-negotiable exit rule

## Current implementation evidence — 2026-07-22 payload sync wave

- Production API now requires S3 configuration in production, injects the S3 remote chunk adapter, and keeps local memory storage limited to non-production/test state.
- Durable sync manifests now persist through the control/PostgreSQL aggregate. Redis is used for lease and fencing validation only on the production API path; manifest reads no longer use Redis as canonical storage.
- Hosted blob GET/PUT enforces resource ACL, immutable server-derived keys, checksum validation, upload size limits, idempotency key presence, and active lease/fencing validation before writes. Manifest commit verifies every referenced blob and size before durable publication.
- Desktop production now requires a stable `BUGLOGIN_DEVICE_ID`, hydrates checksum-verified profile generations into replaceable cache staging, publishes only complete generations atomically, acquires a writer lease, rechecks the manifest under lease, and launches only the verified generation path.
- Production launch/runtime projection and stop write-back/reconcile paths are implemented and unit-covered; release remains blocked pending live S3/Redis interruption evidence, real Wayfern artifact evidence, signed Windows/macOS acceptance, and native macOS evidence.

“Ready implement” means controlled implementation may start; it does not mean production-ready. No production claim until platform E2E, sync interruption, migration rehearsal/rollback, visual approval, security, performance, and signing gates pass.

- Hosted sync recovery now uses two-pass orphan cleanup: stale proven-uncommitted journals enter `CleanupPending` before any delete; a later pass rechecks active manifest references and checksum-CAS deletes only journal-created objects. Referenced/resumed/delete-failed objects remain preserved. Native/live S3 evidence remains pending.


- Hosted profile archive/restore now uses the durable control aggregate for REST and MCP, updates profile plus managed binding atomically, enforces revision/ACL/idempotency, rejects running profiles, and reloads after ambiguous persistence completion. Targeted REST/MCP/running/restart tests and full `cargo test -p buglogin-api` pass.





- Web identity wave update (2026-07-23): durable recovery, trusted HTTPS webhook delivery, scoped API tokens, domain ACL scope enforcement, REST/OpenAPI/generated contracts, `/recover`, and Account Security token UI are implemented. `cargo test -p buglogin-api` (44 unit plus all integration suites), `cargo test -p buglogin-control-app`, `cargo test -p buglogin-migration --tests`, migration rehearsal evidence, web tests/build, contract drift, secret boundaries, and formatting pass. Browser screenshot QA could not run because the browser transport was unavailable. Hosted PostgreSQL auth E2E remains unverified because no `BUGLOGIN_LIVE_POSTGRES_URL`/password was supplied; the local `psql` probe correctly failed closed with `fe_sendauth: no password supplied`.

- Cookie metadata persistence wave (2026-07-23): append-only atomic revisions, restart rehydration, immutable two-slot OS-vault staging, replacement rollback, batch staged-secret cleanup, and zero-value metadata evidence are implemented in `buglogin-browser`; full browser tests pass. Real stopped-profile Wayfern cookie DB materialization, native Windows/macOS vault evidence, adapters/UI, sync interruption, and migration verification remain release blockers.

- Production data-root inspection (2026-07-23): two read-only snapshots of `%LOCALAPPDATA%\BugLogin` matched at `9da4587dead42f0dc4e0a22c25196338292b15ad06ae7fddfcc4a12990013912` (1,961,345,035 bytes, 15,332 files). Manifestless inspect now accepts explicit source identity without writing the source. Actual audit finds 15 `bugmini` profiles and correctly blocks cutover because the adapter is not approved; no control-plane export or destination write was attempted.
- Status correction (2026-08-21): the Rust `bugmini` adapter and API lifecycle dispatch are now implemented and covered by runtime-contract tests. Migration inspection reports the adapter as partial until a verified runtime artifact is provisioned; production cutover still requires signed Windows/macOS evidence, control-plane export, and rehearsal/rollback proof.
- Controlled implementation update (2026-08-21): web workspace proxy inventory now uses the real proxy list/create/update/delete contracts with server-derived revisions, sealed-credential display, idempotency, conflict-safe refresh, and explicit profile-assignment limitation. Web profile operations retain real create/launch/stop/poll flows. Desktop profile workspace no longer displays a hardcoded workspace name; it resolves the first server/local workspace and routes the proxy rail to the existing proxy drawer. Web and desktop targeted typecheck/self-tests pass.
- Migration desktop integration now exposes local-only `migration.inspect` through the existing operation bus and typed Tauri bridge. It requires an explicit `BUGLOGIN_MIGRATION_BINARY`, polls real operation state, fingerprints the source before/after, and production mode rejects the command before any filesystem access. Sidecar packaging, audit proof, import/cutover UI, and signed native executable evidence remain release blockers.
- Desktop local migration now also exposes `migration.import` through the same operation bus. It requires a signed package, explicit dry-run approval, exact active-repository destination, separated staging/package paths, confirmation `CUTOVER_AND_PROMOTE`, sidecar execution, and package immutability verification. The packaged sidecar build script produces a target-triple binary; native signing and two full rehearsals remain open.
- Migration HTTP inspect now returns the stable `{ report, snapshot }` result envelope with regression coverage. Runtime catalog artifact verification is checksum/size-bound and deterministic; no native artifact approval is implied.
- Governance UI update (2026-08-21): web admin now presents live authorized organizations, workspaces, identities, memberships, resources, grants, and policy limits at `/admin/users`; audit/jobs/sync telemetry remains explicitly absent because no read contract exists. Workspace members now support real invite, activate, suspend, and role-update mutations with server ACL, idempotency, refresh, and error states. Team mutation UI remains withheld pending a distinct approved contract path.

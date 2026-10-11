# BugLogin Rebuild Discovery Design

- Date: 2026-07-20
- Status: discovery only; no product implementation approved
- Reference sources:
  - `.tmp/buglogin-legacy`: local clone of `E:\bug-login`; source branch snapshot `main`, working source had untracked files before cloning
  - `.tmp/donutbrowser-upstream`: shallow clone of `https://github.com/zhom/donutbrowser`; `HEAD=a4ed5c855afc4242b6c89c7d73ad767f2357c7f0`, `2026-07-18`; latest stable tag observed: `v0.28.2`, `2026-07-12`

## 1. Decision summary

Rebuild as a **desktop-first, offline-capable, profile-isolation product**. Use DonutBrowser as the upstream core. Do not port the old BugLogin application wholesale.

The new product owns only:

1. Profiles and isolated browser data.
2. A supported anti-detect runtime contract.
3. Proxy/VPN assignment and verification.
4. Groups, tags, notes, bulk operations.
5. Extension and cookie management.
6. Local API/MCP integrations, behind explicit opt-in and authentication.
7. Optional encrypted sync.
8. Safe updates, diagnostics, import, and recovery.

Cloud billing, marketing, admin commerce, TikTok/Etsy automation, cookie farming, anti-bot bypass workflows, and proprietary browser-engine experiments are not part of the v1 desktop core.

## 2. Evidence from the legacy repository

The legacy clone contains 2,047 tracked files:

- 712 documentation files.
- 442 frontend files.
- 376 Rust files.
- 55 sync-server files.
- 84 portal/admin route files plus marketing and desktop surfaces.
- Very large mixed-responsibility files: browser runner, sync engine, profile manager, desktop page, profile table, sync control service.
- Tracked runtime/test artefacts include logs, screenshots/HTML sessions, `__pycache__`, temporary JSON/CSV, and test scripts.
- Rust command surface spans browser launch/park/kill, profile CRUD, multiple runtimes, proxy/VPN, sync, billing/entitlement, cookie workflows, TikTok helpers, MCP, diagnostics, update systems, and workspace locks.

The old repository is therefore a product monorepo plus operations lab, not a clean browser core. Its feature count must not become the new scope.

## 3. Upstream baseline

DonutBrowser currently provides the cleanest baseline for the requested rebuild:

- Tauri 2 desktop shell.
- Next/React frontend with one desktop surface.
- Rust profile/runtime manager.
- Wayfern anti-detect runtime; Camoufox is deprecated upstream.
- HTTP/HTTPS/SOCKS proxy support, dynamic proxy URLs.
- WireGuard VPN support.
- Profile groups, extensions, cookies, imports, default-browser/deep-link handling.
- DNS blocklist.
- Local REST API and MCP server.
- Self-hostable sync with optional E2E encryption.
- Runtime downloader/registry and app/browser update paths.
- OpenAPI coverage tests, Rust integration tests, sync E2E tests, lint/type/test CI.

Reuse upstream behavior where it is already correct. Fork only for BugLogin branding, product defaults, supported runtime policy, UX, migration, and explicitly selected business features.

## 4. Product boundaries

### In scope for v1

| Capability | Decision | Notes |
|---|---|---|
| Unlimited local profiles | Keep | Primary value. Must isolate cookies, storage, extensions, fingerprint config, proxy/VPN state. |
| Wayfern runtime | Keep | Single supported engine initially. Version compatibility is a release gate. |
| Profile CRUD | Keep | Create, edit, clone, archive/delete, restore/recovery. |
| Launch lifecycle | Keep | Launch, stop, park if proven useful, status reconciliation, crash cleanup. |
| Proxy manager | Keep | HTTP/HTTPS/SOCKS4/SOCKS5, auth, test, assignment, bypass rules. |
| WireGuard VPN | Keep selectively | Include only if real users need it; enforce proxy/VPN mutual exclusion clearly. |
| Groups/tags/notes | Keep | Bulk assignment and filtering. |
| Extensions | Keep | Explicit install/update/delete/assignment; no bundled opaque extensions. |
| Cookies | Keep | Import/export/read only through controlled flows; never log cookie values. |
| Browser import | Keep | Chrome/Edge/Brave/Chromium import with dry-run and per-profile result. |
| Default browser/deep links | Keep | Profile chooser and deterministic URL routing. |
| Local REST API | Keep | OpenAPI-first, bearer auth, localhost binding, per-action permissions. |
| MCP | Conditional keep | Opt-in, disabled by default, visible status, scoped tools, audit logs. |
| Sync | Conditional keep | Self-host first; cloud only after contract and encryption are stable. |
| E2E sync encryption | Keep if sync ships | Password-derived key; recovery semantics must be explicit. |
| Updates/diagnostics | Keep | Signed/checksummed assets, rollback-safe updates, redacted export. |
| Multi-language UI | Keep | Vietnamese + English first; translation keys enforced. |
| DNS blocklist | Conditional keep | Opt-in per profile; measurable failure/allowlist behavior. |

### Explicitly out of v1

| Capability | Decision | Reason |
|---|---|---|
| Marketing website inside desktop repo | Drop | Separate deployment/product surface. |
| Billing, Stripe, coupons, invoices | Drop | Not browser-core functionality; creates auth/data/compliance coupling. |
| Super-admin, commerce, abuse/trust consoles | Drop | Separate control-plane product. |
| TikTok/Etsy seller automation | Drop | Workflow-specific, high maintenance, high account-risk surface. |
| Cookie farming/DataDome/captcha automation | Drop | Not required for profile isolation; security/legal/abuse risk. |
| Embedded captcha/automation extensions | Drop | Supply-chain and opaque behavior risk. |
| Bugox/Bugium/Bugmini/Bugthor/Bughelium runtime matrix | Drop initially | Multiple engines caused branching and identity drift. One runtime contract first. |
| MITM bridge and custom CA rewriting | Drop initially | High trust boundary and TLS/privacy risk; re-evaluate only with written requirement. |
| Full OpenVPN/SOCKS tunnel matrix | Drop initially | Keep WireGuard only unless a supported use case proves otherwise. |
| Remote billing-based feature gates | Drop from local core | Entitlements must not break offline profile access. |
| Portal/admin route forest | Drop | Replace with small optional control plane later. |

### Conditional later

- Team workspaces, roles, invites, locks, quotas.
- Cloud subscription and commercial entitlements.
- Remote browser-release control plane.
- Enterprise audit/export policies.
- Automation SDK beyond basic local API.

These require separate specs. Do not smuggle them into the desktop rebuild.

## 5. Required product decisions before coding

1. **Target users:** personal multi-account users, agencies, or internal operators. One primary persona only for v1.
2. **Supported OS:** Windows first or Windows/macOS/Linux together. The legacy release matrix was unstable; define support and signing explicitly.
3. **Runtime:** Wayfern version policy, download source, checksum/signature trust root, update rollback behavior.
4. **Privacy model:** what is local, what syncs, whether profile content is ever uploaded, retention and deletion guarantees.
5. **Sync model:** self-host-only v1 or hosted cloud; conflict policy; whether profile binaries sync or config-only sync.
6. **API model:** local-only API, remote API, or both. Token storage, rotation, scope, rate limits.
7. **VPN scope:** WireGuard-only or no VPN in first release.
8. **Legacy compatibility:** supported source versions and whether cookies/extensions are migrated automatically.
9. **Licensing/branding:** AGPL obligations, attribution, source distribution, trademark/rebrand review, and any upstream written permission requirement. Do not assume a fork is commercially unrestricted.
10. **Abuse policy:** acceptable-use policy, support boundaries, and prohibited automation. Anti-detect features need explicit product/legal review.

## 6. Target architecture

### Process boundaries

```text
Desktop UI (React)
        |
        | typed Tauri commands/events
        v
Rust application core
  - profile domain
  - runtime adapter
  - proxy/VPN
  - extension/cookie import
  - sync engine
  - local API/MCP
  - updater/diagnostics
        |
        +-- local profile directories
        +-- local metadata/settings
        +-- downloaded runtime registry
        +-- optional sync server/object storage
```

Rules:

- Rust is the authority for filesystem, process, network, permissions, state transitions, and secrets.
- React is a projection, not a security boundary.
- Tauri commands remain thin: validate input, call one domain operation, return typed result/error.
- Side effects belong in domain modules/services, not page components.
- Every long operation has progress, cancellation policy, timeout, retry policy, and terminal failure state.
- Every public local API route is represented in OpenAPI and regression-tested.
- One canonical type per domain object. No duplicate “legacy/new” profile types in UI.

### Suggested Rust modules

Keep upstream module boundaries where they are good:

- `profile`: model, persistence, CRUD, clone, lifecycle metadata.
- `runtime`: Wayfern catalog, download, compatibility, launch adapter, process cleanup.
- `network`: proxy, WireGuard, checks, traffic diagnostics.
- `extensions`: extension/group lifecycle and assignment.
- `cookies`: import/export/read with redaction guarantees.
- `sync`: manifest, entity sync, encryption, conflict resolution.
- `api`: local REST/OpenAPI/auth and MCP integration.
- `platform`: default browser, deep links, tray, OS permissions.
- `settings`: preferences, language, update policy.
- `diagnostics`: logs, health, export, recovery.

Do not recreate a giant `lib.rs`, giant `browser_runner.rs`, or page component that owns all state.

### Persistence

Start with upstream persistence formats to minimize migration risk. Add a database only if profiling proves file-based metadata cannot handle the target profile count or atomicity requirements. If a database is later needed, migrate behind a versioned repository boundary; do not mix JSON and DB writes ad hoc.

Mandatory persistence rules:

- Atomic write: temp file + fsync/rename where applicable.
- Version every metadata schema.
- Preserve unknown fields during forward-compatible reads where safe.
- Never delete source data during import.
- Secrets/keys use OS-appropriate protected storage or strict file permissions.
- Runtime state is reconstructible; do not treat `running` or `process_id` as durable truth.

## 7. Domain model contract

Minimum entities:

- `Profile`: stable ID, name, runtime, runtime version, fingerprint config, proxy/VPN reference, group IDs, tags, note, extension group, sync mode, created/updated timestamps, lifecycle state.
- `ProfileData`: isolated browser directory, cookies, local storage, cache/session data, extension data. Large binary data is separate from metadata.
- `Group`: stable ID, name, appearance, timestamps.
- `Proxy`: stable ID, protocol, host/port, credentials reference, rotation/dynamic URL, bypass rules, health result, timestamps.
- `VpnConfig`: stable ID, provider/type, protected config reference, status, timestamps.
- `Extension` / `ExtensionGroup`: stable IDs, source, version, install state, assignment, timestamps.
- `SyncMetadata`: entity version, updated timestamp, encryption state, last successful operation, failure reason.
- `RuntimeInstall`: runtime/version/platform/arch, source, checksum, install state.

State machines must be explicit:

- Profile: `created -> ready -> launching -> running -> stopping -> ready`; `failed` and `recovering` are terminal/repair branches.
- Download: `queued -> downloading -> verifying -> installing -> ready`; `cancelled`/`failed` preserve diagnostics.
- Sync: `idle -> checking -> uploading|downloading|conflict -> completed|failed`.
- Import: `detected -> previewed -> copying -> validating -> completed|partial|failed`.

No UI-derived state may bypass these transitions.

## 8. Legacy migration contract

Migration is a product feature, not a one-off script.

### Required flow

1. Detect supported BugLogin data roots without guessing destructive paths.
2. Require old app closed; detect running profile processes.
3. Create an immutable backup/reference manifest before copying.
4. Dry-run: list profiles, groups, proxies, VPNs, extensions, cookies, unknown fields, and conflicts.
5. Copy, never move. Source remains untouched.
6. Convert metadata through a versioned mapping table.
7. Copy browser data only when runtime compatibility is proven; otherwise import supported cookies/extensions and mark the profile partial.
8. Validate each migrated profile: metadata parse, directory presence, runtime launch, proxy check, cookie persistence.
9. Produce per-item result: migrated, partial, skipped, failed, with reason and recovery action.
10. Keep a rollback manifest. Rollback removes only new destination artifacts created by that migration run.

### Mapping policy

- Preserve IDs when format and collision rules allow; otherwise maintain `old_id -> new_id` mapping.
- Map groups/tags/notes/proxy assignments first, then profiles.
- Treat BugLogin-specific engines/configs as unsupported unless a dedicated adapter is approved.
- Never silently map Bugox/Bugium/Bugmini fingerprint settings to Wayfern. Require regenerated fingerprint config and user-visible warning.
- Never copy access tokens, cloud session files, debug logs, or admin artifacts into the new profile store by default.
- Cookies and extensions require explicit consent because they are credentials/code.
- Unknown fields go to a quarantine report, not into arbitrary runtime config.

### Migration acceptance gates

- Zero source mutation in a migration test.
- Repeated migration is idempotent or produces a deterministic conflict report.
- Crash/restart resumes safely or rolls back safely.
- At least one fixture per OS/source browser/runtime combination.
- Launch succeeds after migration with cookies preserved where supported.
- No secrets in logs, reports, screenshots, or telemetry.

## 9. Security and trust boundaries

High-risk surfaces requiring separate review:

- Browser process launch arguments and environment variables.
- Proxy credentials and dynamic proxy URLs.
- VPN private keys.
- Cookies/session exports.
- Local REST API and MCP server.
- Runtime/browser downloads and update verification.
- Sync object storage and encryption keys.
- Imported extensions.
- Default-browser/deep-link URL handling.
- Any future MITM or traffic rewriting.

Baseline controls:

- Bind local servers to loopback unless explicitly configured.
- Require bearer token for API/MCP; generate random token; support rotation/revocation.
- Do not put secrets in command errors, logs, analytics, crash reports, or UI state snapshots.
- Redact proxy credentials and cookie values everywhere.
- Verify runtime/update checksums or signatures before install.
- Restrict file permissions for secret files.
- Validate all paths against allowed app roots; prevent traversal and symlink escape where relevant.
- Default sync encryption on when profile content leaves the device.
- Make dangerous operations explicit and reversible where possible.
- Add abuse/acceptable-use documentation before public distribution.

## 10. Verification strategy

### Before implementation

- Freeze product decisions in a signed-off capability matrix.
- Freeze supported OS/runtime/version matrix.
- Capture fixture copies of representative legacy profiles without secrets.
- Define API/OpenAPI and migration reports before code.

### Per phase

- Rust unit tests for domain/state transitions.
- Rust integration tests for proxy/VPN/runtime lifecycle.
- Frontend typecheck/lint; component behavior tests only where stateful.
- Sync E2E with two clients, conflict, retry, interruption, and encryption.
- Migration fixture tests with source immutability checks.
- API contract regression tests.
- Manual smoke matrix: create, launch, login, relaunch, stop, proxy failure, crash recovery, update failure, import, sync, delete/restore.

### Release gates

- No critical security findings.
- No data-loss path in migration/update/sync.
- No stuck runtime/process after normal close, crash, or update.
- API spec matches handlers.
- Clean install and upgrade from the previous supported build.
- Signed artifacts/checksums verified on each supported OS.
- Diagnostics contain enough evidence without credentials.

## 11. Proposed phases

### Phase 0 — product freeze

Output: persona, scope matrix, OS/runtime policy, licensing/abuse decision, data contract, threat model.

### Phase 1 — upstream baseline

Output: branded-but-minimal Donut-derived app builds, one runtime, profile CRUD/launch/stop, proxy, groups, settings, update path.

### Phase 2 — hardening

Output: typed command boundary, state machines, process cleanup, API/OpenAPI, diagnostics, security review, test fixtures.

### Phase 3 — migration

Output: dry-run importer, backup/rollback, legacy profile fixtures, partial migration UX, idempotency.

### Phase 4 — integrations

Output: extensions/cookies/import polish, optional WireGuard, local API/MCP scopes, DNS blocklist.

### Phase 5 — sync

Output: self-host sync, E2E encryption, conflict/recovery, two-client E2E tests, operational docs.

### Phase 6 — optional control plane

Output: workspace/RBAC/cloud/billing only as a separate approved product, not mixed into desktop core.

## 12. Open questions blocking implementation

- Who is the primary user and what exact workflow pays for BugLogin?
- Is v1 Windows-only? If not, who owns macOS/Linux signing and support?
- Is Wayfern legally/technically acceptable as the only runtime?
- Must old BugLogin profiles launch unchanged, or is supported-data import enough?
- Is profile-directory sync required, or only metadata/proxy/group sync?
- Is cloud hosting required at launch, or self-host sync sufficient?
- Which API/MCP actions are safe enough to expose?
- Is VPN truly needed in v1?
- Which languages are launch-critical?
- What data must never leave the device?
- What is the commercial/license/branding relationship with upstream?
- What is the explicit acceptable-use and abuse response policy?

Until these are answered, implementation would create avoidable rework. The next action is a product decision workshop, not code.

## 13. Clarification addendum — expanded BugLogin direction

The new requirements expand the target from “clean Donut desktop fork” to a **BugLogin operations platform built around a clean browser core**. The scope is valid, but it must be split into bounded domains. Otherwise the old failure repeats: NestJS, Rust, Next server routes, desktop UI, sync, billing, and automation become one distributed half-system.

### 13.1 Product shape

```text
BugLogin Platform
├── Browser Core
│   ├── profiles
│   ├── browser runtimes: legacy + Wayfern
│   ├── proxies / VPN
│   ├── cookies / extensions
│   └── local automation surface
├── Operations Control Plane
│   ├── organization / workspace
│   ├── teams / members / roles
│   ├── resources / grants / shares
│   ├── admin / HR / audit
│   └── quotas / policy / feature access
├── Data Plane
│   ├── profile metadata
│   ├── browser profile files
│   ├── cookies / extensions
│   ├── sync manifests / chunks
│   └── backup / restore / retention
└── Automation Plane
    ├── typed command registry
    ├── REST/OpenAPI
    ├── MCP tools
    └── progress/events/audit
```

The browser core must run offline. The control plane may be unavailable without preventing local profile launch, except for explicitly shared or policy-controlled resources.

### 13.2 Less fragmentation: recommended implementation topology

Use a **Rust modular monolith** for the authority layer, with one frontend package:

```text
buglogin/
├── apps/desktop              # Tauri shell + static React UI
├── apps/control              # same Rust server binary, optional hosted mode
├── crates/domain             # IDs, entities, permissions, state machines
├── crates/browser            # runtime adapters and lifecycle
├── crates/network            # proxy/VPN/traffic checks
├── crates/storage            # SQLite metadata + filesystem/blob adapters
├── crates/sync               # manifests, chunks, encryption, conflicts
├── crates/api                # REST/OpenAPI + auth + MCP registry
├── crates/migration          # legacy import/copy/clone/rollback
└── frontend/                 # React UI; no business authority
```

Recommended simplifications:

- Remove NestJS from the new system unless an existing hosted deployment makes removal impossible.
- Keep Next only as frontend tooling/static build; do not put business authority in Next route handlers.
- Use Rust `axum` or the existing Rust HTTP stack for hosted control/sync APIs.
- Use one shared domain/permission/schema crate between desktop and server.
- Use generated TypeScript types/client from OpenAPI instead of hand-maintained duplicated DTOs.
- Keep one deployable control binary; add Postgres/S3 adapters only when scale requires them.
- Avoid microservices, queues, and Redis until a measured requirement exists.

This is a target architecture decision, not permission to immediately rewrite upstream. First prove the boundary with a vertical slice.

### 13.3 Proxy Parse — first-class fast path

Proxy entry must be a reusable parser library, not UI-only string splitting.

Accepted forms:

- `host:port`
- `host:port:user:password`
- `user:password@host:port`
- `http://user:password@host:port`
- `https://...`, `socks4://...`, `socks5://...`
- newline, comma, semicolon, tab, and clipboard batches
- CSV/JSON with documented column aliases
- IPv4, hostname, and bracketed IPv6

Pipeline:

1. Read clipboard/file/text.
2. Tokenize lines/records without destroying passwords containing delimiters.
3. Parse and normalize protocol, host, port, username, password.
4. Canonicalize for deterministic deduplication.
5. Validate syntax synchronously.
6. Return valid, invalid, duplicate, and ambiguous records separately.
7. Persist only after preview/confirmation or explicit “quick add” mode.
8. Run health checks asynchronously with bounded concurrency.
9. Stream progress and partial results; one bad proxy must not fail the batch.
10. Redact credentials in errors, logs, exports, and MCP output unless the caller has an explicit secret-read scope.

Required UX:

- Paste-and-preview in one action.
- Auto-detect format with manual override.
- Column mapping for CSV.
- Duplicate policy: skip/update/keep-both.
- Protocol default setting, never silent protocol guessing when ambiguous.
- Import result with row numbers and actionable errors.
- Quick add remains reversible.
- Benchmark/check is separate from parse; parse must never wait on network health.

Acceptance targets must be measured, not claimed: parse a large clipboard batch without UI freeze, preserve all valid records, deterministic repeated import, and no secret leakage.

### 13.4 Admin / HR / Resource / Workspace / Team / Member

These are separate control-plane domains, not sidebar labels.

#### Organization

Top-level billing/legal/security boundary. Owns workspaces, policies, retention, audit, and members.

#### Workspace

Operational boundary for profiles and resources. Defines runtime policy, allowed integrations, sharing policy, quotas, and data residency.

#### Team

A group of members with a purpose. A team can receive resources through grants; team membership changes must affect access immediately.

#### Member

A human identity with organization membership, workspace membership, status, role, capabilities, sessions, and audit history. HR data must be minimal; do not store unnecessary personal information.

#### Resource

Every shareable object gets a typed resource ID and owner/scope:

- profile
- profile data/session
- proxy
- VPN config
- group
- extension/extension group
- cookie set
- runtime entitlement
- automation/API credential

#### Admin

Admin is a capability set, not one omnipotent boolean. Separate platform administration, organization administration, workspace operations, resource sharing, billing, audit, and support impersonation. Every privileged action requires server-side authorization and an audit event.

Minimum roles/capabilities:

- organization owner
- organization admin
- workspace owner
- workspace admin
- team lead
- operator
- reviewer/read-only
- member
- external guest
- support auditor

Use capability checks on resource/action pairs. Do not encode access only as role-name conditionals in frontend code.

Required control-plane features:

- invite, accept, revoke, suspend, restore member
- workspace/team membership
- role/capability assignment
- resource ownership and grants
- profile share/revoke
- audit log with actor, target, action, before/after summary, request ID
- session/device management
- policy and quota evaluation
- backup/export and deletion/retention policy

HR-specific features need a separate data/privacy review. “HR” should initially mean member lifecycle and access governance, not payroll or sensitive employee records.

### 13.5 Profile sharing

Sharing is an access grant, not copying a profile directory blindly.

Required grant fields:

- resource ID and resource type
- grantor and grantee
- capabilities: view, launch, edit metadata, edit browser data, manage proxy, export cookies, clone, delete
- expiry and revocation timestamp
- optional team scope
- concurrency policy
- audit correlation ID

A shared profile needs a lease/lock model:

- one canonical `ProfileResource(profileId, resourceId, workspaceId)` binding; display names and matching string IDs are never identity joins;
- one active writer lease per profile data directory;
- read-only/view mode when another user owns the lease;
- heartbeat, expiry, explicit release, and crash recovery;
- conflict report instead of silent overwrite;
- no simultaneous sync of the same mutable browser directory without a defined merge strategy.

`local` profiles remain launchable without the hosted control plane only in the `local` self-hosted artifact. The `production` artifact requires the hosted HTTPS API for profile metadata, permissions, proxy configuration, manifests, runtime projections, mutations, audit, and sync; API failure never falls back to local authority. Managed/shared profiles require the canonical binding and a valid writer lease before any mutable launch or sync write. Missing binding, unavailable lease authority, renewal failure, stale fencing token, revocation, or tenant mismatch removes write authority immediately. Lease tokens remain Rust-internal and are never transported through React, REST payloads, OpenAPI, or MCP tool arguments.

Separate three operations that the old product tends to conflate:

1. **Share access:** same logical profile, controlled permissions.
2. **Clone profile:** new profile ID, selected metadata/data copied.
3. **Export profile:** portable archive with explicit secret/cookie choice.

### 13.6 Legacy browsers plus Wayfern

Use a runtime adapter contract:

```text
BrowserRuntimeAdapter
├── identify(version/platform/arch)
├── install/update/verify
├── create_profile_data
├── launch/stop/park
├── apply_proxy_or_vpn
├── apply_fingerprint_config
├── import/export_supported_data
├── health/diagnostics
└── capabilities
```

Runtime catalog entries declare capabilities instead of pretending parity:

- fingerprint controls
- mobile emulation
- extension support
- cookie import/export
- proxy protocols
- VPN compatibility
- default-browser support
- sync compatibility
- profile-data format/version

Wayfern is the primary new runtime. Legacy browsers are compatibility adapters with an explicit support matrix and deprecation dates. Do not share one fingerprint schema blindly across engines. Store engine-specific config under a versioned adapter payload.

Every profile must record runtime identity and compatibility. A runtime upgrade that changes fingerprint, cookie, extension, or profile format requires migration/rollback behavior.

### 13.7 MCP for every new capability

Use one typed command registry as the source of truth:

```text
CommandDefinition
├── stable name/version
├── input schema
├── output schema
├── required capability scope
├── mutating/destructive classification
├── confirmation policy
├── progress/event contract
├── audit event type
└── adapters: Tauri, REST, MCP
```

Every new business capability must register here before UI work. Generate or derive:

- Tauri command
- REST/OpenAPI operation
- MCP tool schema
- frontend client type
- permission mapping
- audit mapping

MCP is not a bypass around authorization. It uses the same server-side capability checks as UI/API. Destructive operations require explicit confirmation or a caller-provided approval token. Secret-returning operations require a separate scope and must be disabled by default.

MCP minimum behavior:

- list capabilities/tools by caller scope;
- stable versioned names;
- idempotency keys for create/import/sync actions;
- progress for long jobs;
- cancellation/timeouts;
- structured errors;
- audit record for every mutation;
- redaction by default;
- no hidden browser automation outside registered commands.

### 13.8 PRO bypass / unlock policy

Product-level feature access can be opened for BugLogin, but this is not the same as deleting safety controls.

Implement an explicit entitlement policy:

- `BugLogin` product plan has all approved capabilities enabled.
- No remote subscription call is required for local core features.
- No fake “PRO” UI gates remain in the desktop core.
- Commercial/hosted entitlements remain separate from local capability authorization.
- Upstream paid-feature code must be reviewed for license, trademark, and security implications before modification.

Keep safety controls:

- destructive delete/export confirmation;
- profile running/lock protection;
- shared-profile writer lease;
- cookie/credential secret confirmation;
- update checksum/signature verification;
- API/MCP authentication;
- path traversal and process-isolation checks;
- crash recovery and data-loss prevention.

Remove paywall friction and unnecessary warning duplication. Do not remove warnings that prevent irreversible data loss, credential exposure, unsafe downloads, or concurrent-write corruption. Create a warning taxonomy: informational, recoverable, security-critical, irreversible.

### 13.9 Sync redesign — fast, light, reliable

Sync must be local-first and data-plane aware.

Recommended design:

- SQLite metadata/control state in the Rust control binary.
- Content-addressed profile file chunks, compressed and encrypted before upload.
- Incremental manifest: path, size, hash, mtime, content type, chunk list.
- Resumable parallel transfer with bounded concurrency.
- Deduplication across profiles and versions where safe.
- Small config entities sync as versioned JSON records.
- Profile binary data sync only on explicit policy; default metadata/config first.
- One conflict algorithm per entity: `updated_at` plus revision/actor; never use `last_sync` as edit truth.
- Per-profile lease prevents two writers from mutating the same directory.
- Retry with backoff, checksum verification, journaled progress, cancellation, and crash resume.
- Tombstones and retention prevent deleted data from resurrecting.
- Garbage collection is mark-and-sweep with grace period, never immediate blind delete.
- Local offline queue makes transient server failure non-blocking.

Server topology:

```text
Rust control binary
├── SQLite/Postgres metadata adapter
├── object-store adapter (local/S3-compatible)
├── auth/ACL/audit
└── sync HTTP API
```

Do not require NestJS + separate Rust daemon + Next API + Redis for v1. Add Postgres/object storage only behind adapters when deployment scale demands it.

Sync test matrix must include: offline edits, same-object edits, profile-directory concurrent launch, interrupted upload/download, checksum mismatch, revoked share, expired lease, deleted object, server restart, key/password loss, migration-created IDs, and old-client compatibility.

### 13.10 Profile creation UX

Create profile as a guided workflow with safe defaults, not a giant form.

Suggested steps:

1. Name + optional template.
2. Runtime + version with capability summary.
3. Proxy paste/import with instant parse preview.
4. Location/language/timezone policy with consistency preview.
5. Cookies/extensions import, optional.
6. Advanced fingerprint/runtime settings, collapsed.
7. Sharing/group assignment, optional.
8. Review diff + create.

Presets:

- clean profile;
- clone existing profile;
- import browser profile;
- platform preset (user-defined, not hardcoded anti-bot bypass);
- proxy batch preset;
- team template.

Rules:

- sensible defaults from OS/locale;
- validate cross-field consistency before create;
- show only settings supported by selected runtime;
- preserve advanced values on back/forward;
- support keyboard and bulk creation;
- never silently overwrite an existing profile;
- show estimated disk/runtime requirements;
- make post-create launch optional and explicit.

### 13.11 Additional capabilities likely missing

Add to discovery backlog, not automatically v1:

- backup/restore and profile trash with retention;
- global search, saved filters, bulk actions, keyboard shortcuts;
- job center for imports, sync, downloads, checks, migrations;
- notifications and actionable error center;
- health dashboard for runtime/proxy/sync/storage;
- per-profile data export with secret/cookie selection;
- storage quotas and disk cleanup;
- audit viewer and downloadable audit records;
- session/device/token revocation;
- extension trust/source policy;
- runtime rollback and compatibility pinning;
- disaster recovery runbook for hosted control plane;
- schema/version compatibility between desktop clients and server;
- feature flags with local-safe defaults;
- rate limits and abuse controls for API/MCP;
- observability with redaction and correlation IDs;
- accessibility and keyboard-only QA;
- localization completeness checks;
- support bundle that excludes credentials by construction.

### 13.12 Revised non-negotiable architecture rules

1. One authority for permissions: Rust control/domain layer.
2. One typed command registry for UI, REST, Tauri, and MCP.
3. One sync engine; no duplicate sync logic in frontend/server/daemon.
4. One resource/ACL model for UI, API, MCP, and sharing.
5. One profile lifecycle state machine.
6. One runtime adapter contract; engine-specific capabilities explicit.
7. One migration pipeline with dry-run, copy-only, report, and rollback.
8. No entitlement check may block approved local BugLogin core functionality.
9. No safety-critical lock/warning may be removed without a replacement invariant.
10. No new domain feature is complete until its schema, permission, API, MCP, audit, UI, test, and migration impact are specified.

## 14. Revised implementation sequence

### Gate 0 — scope and legal freeze

Approve the expanded product map, licensing/branding, acceptable-use, primary user, OS, runtime policy, and the fixed deployment split: `local` self-hosted authority versus `production` hosted HTTPS API authority.

### Gate 1 — vertical core slice

Implement only a disposable proof: create profile -> parse proxy -> launch Wayfern/legacy adapter -> stop -> inspect diagnostics -> invoke the same action through MCP. Measure process, memory, latency, and error behavior.

### Gate 2 — shared resource/control slice

Add organization/workspace/team/member/resource/grant data model, one role matrix, one profile share flow, one lease, one audit event. Verify server authority and offline behavior.

### Gate 3 — sync slice

Sync one profile metadata record and one small profile-data fixture with resumable transfer, conflict, encryption, revoke, and crash resume. Do not build the whole sync surface before this passes.

### Gate 4 — migration slice

Migrate a real anonymized legacy fixture: profile, cookies, proxy, group, extension, and unsupported engine report. Verify source immutability and rollback.

### Gate 5 — breadth

Add remaining profile/proxy/group/cookie/extension/VPN/DNS/admin/HR/resource flows through the same contracts. Add MCP at feature creation time, not afterward.

### Gate 6 — release hardening

Run performance, security, multi-user sharing, sync failure, upgrade, migration, and cross-runtime QA before production.

## 15. Quality contract — chống thiếu, stub, xấu, lỗi lan truyền

The project must assume that an AI coding agent will otherwise optimize for “code exists” rather than “product works”. These gates are mandatory.

### 15.1 Completeness traceability

Create a capability registry before implementation. Each capability has:

```text
Capability ID
├── legacy evidence / user need
├── v1 decision: keep, replace, later, drop
├── domain owner
├── data schema/migration impact
├── permission/role matrix
├── state machine
├── Tauri/API/MCP contract
├── desktop UI surface
├── web UI surface
├── unit/integration/E2E tests
├── docs/help text
└── release acceptance evidence
```

No capability is “done” when only the backend exists. No UI ticket can close without confirming data, permissions, API, MCP, audit, errors, tests, and migration impact.

Run a legacy parity review by domain, not by file:

- profiles and lifecycle;
- proxy parsing/checking/assignment;
- browsers/runtimes;
- cookies/extensions/import/export;
- groups/tags/bulk actions;
- sharing/locks/permissions;
- sync/backup/restore;
- admin/HR/workspace/team/member/resource;
- settings/update/diagnostics;
- local API/MCP.

Every legacy function receives one explicit disposition. “Not yet reviewed” blocks the corresponding release.

### 15.2 No-stub rule

Forbidden in release code:

- placeholder pages;
- buttons that do nothing;
- fake success toasts;
- hardcoded demo records;
- `TODO`, `FIXME`, `not implemented`, or empty handlers for approved capabilities;
- disabled actions without a visible reason and documented condition;
- UI that hides an authorization/data error;
- “coming soon” replacing a promised feature;
- mock API responses outside isolated tests.

Allowed only in a clearly named prototype branch/screen, never in a release candidate. A feature can be deferred; it cannot be silently stubbed.

### 15.3 Complete UI state contract

Every app and web screen must implement and visually review:

- loading/skeleton;
- first-use empty state;
- populated state;
- validation error;
- server/network error;
- permission denied;
- offline/degraded mode;
- partial success/batch result;
- destructive confirmation;
- concurrent lock/read-only state;
- long-running progress/cancel;
- success/retry feedback;
- narrow desktop and wide desktop layout;
- keyboard navigation/focus/shortcuts;
- reduced motion and readable contrast.

Tables/forms/dialogs must handle long names, large lists, no data, many data, slow network, expired session, and stale updates. No state may rely on browser console output.

### 15.4 UI/UX production process

The UI process is design-first, not JSX-first:

1. Define a product-wide `DESIGN.md`: information hierarchy, density, typography, tokens, color, spacing, surfaces, table/form/dialog patterns, motion, accessibility.
2. Define task flows and low-fidelity wireframes for each major screen.
3. Approve one reference screen per surface: desktop profiles, profile creator, proxy manager, share/access, admin/workspace, sync/job center, web member/resource management.
4. Build shared primitives before page-specific markup.
5. Implement real data and all state variants.
6. Capture screenshots at fixed desktop/web viewports.
7. Run visual review against the approved reference and the local UI rules.
8. Run interactive browser QA: clicks, keyboard, dialogs, filters, pagination, errors, refresh, responsive behavior.
9. Fix visual findings before adding the next screen.

Design quality is measured by evidence: screenshots, interaction recordings or reproducible steps, console/network cleanliness, and accessibility checks. “Looks fine” is not evidence.

Avoid generic AI UI patterns:

- no decorative gradients for internal operations screens;
- no excessive nested cards;
- no giant empty hero areas in admin tools;
- no random one-off colors or typography;
- no duplicate page titles/toolbars;
- no full-width controls when content-width is sufficient;
- no inconsistent Select/Combobox alignment;
- no inaccessible icon-only actions;
- no layout that depends on hidden overflow.

### 15.5 Bug prevention gates

Each vertical slice must pass, in order:

1. Rust unit tests for parsing/domain/state transitions.
2. Rust integration tests for filesystem/process/network behavior.
3. API/OpenAPI contract tests.
4. MCP schema/permission/audit tests.
5. Frontend typecheck/lint.
6. Component/interaction tests for stateful UI.
7. Browser E2E for the real user flow.
8. Visual screenshot review.
9. Cross-role and failure-path tests.
10. Upgrade/migration impact check.

Shared behavior requires broader regression tests. A failing gate blocks dependent work; do not paper over it with a UI conditional.

### 15.6 Test matrix

Minimum matrix:

- OS: every supported OS, plus clean install and upgrade.
- Runtime: Wayfern plus every supported legacy adapter.
- Network: online, offline, slow, timeout, proxy failure, server restart.
- Identity: owner, admin, team lead, operator, member, read-only, guest, revoked member.
- Data: empty, small, large, duplicate, corrupted, partially migrated, stale, locked.
- Operations: create, edit, launch, stop, clone, share, revoke, import, export, sync, restore, delete.
- Interfaces: desktop UI, web UI, REST, MCP.

### 15.7 Release discipline

- Work in vertical slices, not a giant rewrite branch.
- Each slice has a demoable checkpoint and an acceptance report.
- No more than one domain is changed without passing its current gates.
- Freeze new features before release hardening.
- Run exhaustive QA on release candidate, not only unit tests.
- Record known issues with severity, repro, impact, workaround, and owner.
- Do not call a release production-ready if any critical/high data-loss, permission, runtime, or blocking UX issue remains.

### 15.8 Agent operating rules

Codex must:

- read the capability registry before coding;
- never invent UI patterns when a shared component exists;
- never implement a page without loading/empty/error/permission states;
- never claim visual quality without screenshots and interaction checks;
- never close a feature without MCP/API/permission/test review;
- stop and report when scope or architecture becomes ambiguous;
- prefer deletion or deferral over a fake implementation.

The user reviews checkpoints, not a final surprise. The project remains in discovery until Gate 0 is approved.

## 16. Production migration plan — BugLogin 1.x to 2.0

Production data is not one thing. Migration must cover four planes separately:

| Plane | Legacy evidence / source | Risk |
|---|---|---|
| Local metadata | `BugLogin/profiles/{uuid}/metadata.json`, `proxies/{id}.json`, `data/tags.json`, `data/extension_groups.json`, `vpn/vpn_configs.json`, `settings/*.json` | Schema drift, stale runtime state |
| Browser data | `BugLogin/profiles/{uuid}/profile/` | Cookies, sessions, local storage, browser-format compatibility |
| Control plane | users, organizations/workspaces, teams, members, shares, locks, entitlements, audit data | Permission/data ownership loss |
| Object/sync plane | remote manifests, profile blobs/chunks, sync metadata, encryption metadata | Partial transfer, resurrection, key loss |

Tokens, proxy passwords, VPN private keys, cookies, cloud sessions, and extension packages are secrets. They require separate inventory, access control, redaction, and verification.

### 16.1 Principle: old system remains recoverable

Never transform the only production copy in place.

- Keep BugLogin 1.x source data untouched.
- Run 2.0 against a new data root, database/schema, object-store prefix, and encryption namespace.
- Never point 2.0 directly at the old database or bucket as its writable store.
- Never let 2.0 “clean up” unknown files in the old root.
- Preserve old application binaries and deployment manifests.
- Retain the old production environment until the rollback window expires.

### 16.2 Migration tooling contract

Build a dedicated Rust migration tool before production cutover:

```text
buglogin-migrate
├── inspect       # read-only inventory and health
├── snapshot      # immutable manifest + source checksums
├── dry-run       # mapping, conflicts, unsupported data
├── export        # versioned neutral package
├── import        # idempotent destination import
├── verify        # counts, hashes, permissions, launch samples
├── cutover       # controlled switch/lock release
└── rollback      # restore service routing, never delete source
```

Every command supports:

- explicit source/destination IDs;
- `--read-only` where applicable;
- structured JSON report plus human summary;
- correlation/run ID;
- resumable checkpoints;
- idempotency key;
- no secrets in normal logs;
- no destructive default;
- exit code that blocks cutover on critical errors.

### 16.3 Neutral export package

Do not make 2.0 read every old internal format forever. Convert 1.x into a versioned neutral package:

```text
buglogin-export-v1/
├── manifest.json
├── identities/
├── organizations/
├── workspaces/
├── teams/
├── members/
├── grants/
├── profiles/
│   └── {old-profile-id}/
│       ├── metadata.json
│       ├── browser-data/
│       └── migration-report.json
├── proxies/
├── vpns/
├── groups-tags/
├── extensions/
├── sync-manifests/
├── object-references/
├── secrets/               # encrypted, separately permissioned
└── checksums.json
```

The package is a migration artifact, not a backup replacement. Store it encrypted, access-controlled, time-limited, and outside the application source tree.

### 16.4 Three-pass migration

#### Pass 1 — inventory, no writes

- Stop only the migration job, not production yet.
- Discover all active local roots and remote tenants.
- Count entities and bytes.
- Detect malformed metadata, duplicate IDs, orphaned browser directories, missing proxy/VPN references, stale locks, failed sync records, unsupported runtime configs, and unreferenced objects.
- Record active sessions/processes.
- Produce a blocking issue list.

#### Pass 2 — rehearsal on a production clone

- Clone database/control-plane state into an isolated environment.
- Clone object data into a separate bucket/prefix.
- Copy representative local profile fixtures, including large and damaged cases.
- Run export/import/verify repeatedly.
- Launch a sample of every runtime/profile type.
- Test cookies, local storage, extensions, proxy, sharing, locks, sync, REST, MCP, admin roles, and rollback.
- Compare reports across runs; results must be deterministic.

#### Pass 3 — production cutover

Recommended first cutover uses a short write freeze, not dual-write:

1. Announce maintenance window and recovery plan.
2. Disable new writes in old control plane; keep read-only status visible.
3. Stop old desktop clients or force a versioned maintenance response.
4. Wait for browser, proxy, VPN, sync, and queued jobs to terminate or be marked.
5. Take database snapshot and object-store versioned snapshot.
6. Generate immutable source manifest/checksums.
7. Export old data.
8. Import into isolated 2.0 environment.
9. Run blocking verification gates.
10. Switch API/domain/config routing to 2.0.
11. Re-authenticate clients only where required; never silently discard local tokens.
12. Enable writes gradually for internal operators first.
13. Monitor errors, profile launch, sync, permissions, and storage.
14. Keep old source read-only for the rollback retention period.

Do not attempt live dual-write in the first migration. It creates conflict resolution between two different schemas and two different business authorities. Add CDC/dual-write only if a later zero-downtime requirement justifies the complexity.

### 16.5 Entity mapping

Preserve IDs when safe; always retain mapping tables:

```text
mapping_run_id
old_system
old_entity_type
old_id
new_entity_type
new_id
mapping_status
reason
source_checksum
created_at
```

Mapping order:

1. organizations/tenants;
2. users/members;
3. workspaces/teams;
4. groups/tags;
5. proxies/VPNs/extensions;
6. profiles and runtime config;
7. grants/shares/locks;
8. sync manifests/object references;
9. settings/preferences;
10. audit/history/diagnostics where retention policy allows.

Rules:

- Do not match users by email alone when an immutable legacy ID exists.
- Require manual review for duplicate email/identity matches.
- Do not migrate expired runtime locks as active locks; convert them to historical events.
- Do not migrate process IDs, running flags, temporary ports, cache files, proxy health caches, or old queue state as live state.
- Do not migrate billing entitlement as authority if BugLogin 2.0 opens approved core features; retain it only as historical/reference data.
- Map old workspace/team/resource grants to explicit 2.0 capabilities.
- Preserve audit records when legal/retention policy permits; do not invent actor identity for un-attributed records.

### 16.6 Browser profile data strategy

For each profile, classify:

- **Direct-compatible:** copy browser data directory, rewrite only destination path/metadata, launch and verify.
- **Adapter-compatible:** copy supported data, apply runtime-specific migration, verify.
- **Partial:** import cookies/extensions/bookmarks where supported; keep unsupported files in an encrypted archive.
- **Unsupported:** retain source untouched and show a manual recovery/export path.

Never silently convert a Bugox/Bugium/Bugmini/Bugthor/Bughelium fingerprint payload into Wayfern settings. Keep old runtime adapters where required; generate a new Wayfern config only with explicit user-visible status.

Cookie/session caveats:

- Same-machine migration may preserve OS-bound cookie encryption more reliably than cross-machine migration.
- Cross-OS or cross-user migration can make encrypted cookies unusable even when files copy correctly.
- Verification must test actual relaunch/login/session persistence, not just file presence.
- Cookie export/import must be explicit, encrypted, redacted in logs, and permissioned.

### 16.7 Sync/object migration

- Freeze source sync writes before snapshot.
- Copy manifests before objects.
- Verify every referenced object by size and hash.
- Copy objects into a new 2.0 namespace; do not mutate old keys.
- Preserve encryption metadata and key version.
- Re-encrypt only through a versioned, verified job with old-key fallback during rollback.
- Import tombstones before live objects to prevent deleted data resurrection.
- Rebuild derived indexes after verified object import.
- Mark all imported data with migration run ID and source revision.
- Garbage-collect neither source nor imported objects until retention expires.

### 16.8 Verification gates before enabling writes

Cutover blocks when any critical check fails:

- entity counts reconcile or every difference has an approved reason;
- every profile has a migration result;
- profile IDs, group assignments, proxy/VPN assignments, and grants reconcile;
- object manifests and checksums verify;
- sampled profiles launch on intended runtime;
- cookies/extensions pass explicit persistence tests where supported;
- admin/member/workspace permissions pass allow/deny matrix;
- shared profile lease/revoke works;
- REST and MCP actions honor the same ACL;
- no active old process can mutate after freeze;
- no credentials appear in reports/logs;
- backup restore succeeds in an isolated environment;
- monitoring and rollback switch are tested.

Use both counts and sampled functional tests. Counts alone cannot prove browser sessions work.

### 16.9 Rollback

Rollback means routing writes back to BugLogin 1.x and restoring its known-good deployment, not deleting 2.0 files.

- Keep 1.x database/object snapshots immutable.
- During the rollback window, record 2.0 writes separately.
- If rollback occurs, declare 2.0 writes divergent; do not silently merge them into 1.x.
- Export 2.0 post-cutover changes into a recovery package for manual reconciliation.
- Restore old API/client routing and validate profile launch, sync, and permissions.
- Remove 2.0 only after explicit approval and retention expiry.

Recommended initial retention: at least 30 days, adjusted by data volume, legal policy, and recovery testing.

### 16.10 Migration is complete only when

- rehearsal passed twice with deterministic output;
- production freeze/cutover completed with signed report;
- all critical/high discrepancies resolved or accepted by named owner;
- internal operators used 2.0 successfully;
- rollback was tested before public enablement;
- support has profile recovery instructions;
- old data remains recoverable through the retention period;
- final migration package, mapping table, checksums, and residual risks are archived securely.

## 17. Web 2.0 — full replacement, not UI migration

The legacy web surface is not a preservation target. Its routes, page components, billing/admin trees, duplicated API clients, and visual patterns can be discarded. We migrate **identity, authorization, resources, audit, and billing records where required**; we do not migrate the old web UI.

### 17.1 Web product surfaces

#### Public site

Purpose: explain product, establish trust, support download/onboarding.

- landing page;
- product capabilities;
- runtime/privacy/security explanation;
- download/install;
- documentation/help;
- status/incident page;
- legal/privacy/acceptable-use pages;
- contact/support.

Public web is not the desktop dashboard. It can be visually expressive, but must not become a marketing wrapper around unfinished product behavior.

#### Identity and account console

Purpose: manage BugLogin account identity and security.

- sign up/sign in/sign out;
- email/OTP/OAuth only if approved;
- password/security settings;
- active sessions/devices;
- revoke sessions/tokens;
- personal profile and locale;
- API/MCP tokens and scopes;
- invitations/notifications;
- account data export/deletion requests;
- subscription/plan display only if commercial billing is approved.

#### Workspace operations console

Purpose: manage organization resources and people; browser runtime remains primarily desktop.

- organization/workspace switcher;
- workspace overview and health;
- teams and members;
- roles/capabilities;
- resources: profiles, groups, proxies, extensions, VPNs;
- sharing/grants/revocation;
- locks/active sessions;
- sync status/jobs/errors;
- audit log;
- quotas/storage/retention;
- workspace settings and policies.

#### Platform admin console

Purpose: operate BugLogin itself, not ordinary customer work.

- users and memberships;
- organizations/workspaces;
- support/audit access;
- runtime/browser release catalog;
- extension/source policy;
- incidents and health;
- abuse/acceptable-use review;
- feature configuration;
- data governance/retention;
- system jobs and migration status.

Billing/commerce remains a separate approved module. It must not contaminate the browser and workspace core.

### 17.2 Web architecture rules

- One React + TypeScript design system for public, account, workspace, and admin surfaces; separate shells, shared primitives.
- Static frontend build where possible; Rust control server owns business APIs.
- No Next API routes, server actions, or frontend-only authorization.
- All data access uses generated OpenAPI clients.
- All mutations use the same command registry, ACL, audit, idempotency, and error taxonomy as desktop and MCP.
- Web sessions use secure server-managed session/cookie handling; desktop device login uses a separate explicit device flow.
- No credential/token values in URL query parameters, screenshots, telemetry, or client logs.
- Workspace/admin routes must return server-enforced `401/403`; hiding a menu item is UX only.
- Public pages may be SEO-oriented; operational pages must be dense, clear, responsive, keyboard-accessible, and low-noise.

### 17.3 Web information architecture

Do not reproduce the legacy route forest. Use a small stable hierarchy:

```text
Public
├── /
├── /download
├── /docs
├── /security
├── /status
└── /legal/*

Identity
├── /auth/sign-in
├── /auth/sign-up
├── /auth/verify
├── /account/profile
├── /account/security
├── /account/sessions
└── /account/tokens

Workspace
├── /workspaces
├── /workspaces/:id/overview
├── /workspaces/:id/members
├── /workspaces/:id/teams
├── /workspaces/:id/resources
├── /workspaces/:id/shares
├── /workspaces/:id/sync
├── /workspaces/:id/audit
└── /workspaces/:id/settings

Platform admin
├── /admin/overview
├── /admin/users
├── /admin/organizations
├── /admin/workspaces
├── /admin/runtimes
├── /admin/jobs
├── /admin/incidents
├── /admin/audit
└── /admin/policies
```

Routes are proposals, not implementation commitments. Domain names and permission boundaries are the source of truth.

### 17.4 Web redesign process

1. Audit the old web only for data, user jobs, permission rules, and broken workflows.
2. Delete old visual assumptions from the design baseline.
3. Define `DESIGN.md` and separate visual modes: public editorial, account utility, operations console, platform admin.
4. Create task flows before page lists.
5. Build a clickable reference prototype for the six highest-risk flows:
   - sign-in/session recovery;
   - workspace/member invitation;
   - role/capability edit;
   - profile share/revoke;
   - sync/error recovery;
   - admin user/workspace intervention.
6. Review typography, hierarchy, density, responsive behavior, accessibility, and error states.
7. Implement real APIs, not mock data.
8. Run screenshot and interaction QA at fixed desktop/tablet/mobile viewports.
9. Only then expand breadth.

### 17.5 Web migration rules

- Preserve account IDs, organization/workspace IDs, member IDs, role assignments, resource IDs, grants, audit history, and required billing references through the platform migration.
- Do not preserve old route IDs or frontend state unless needed for redirects/support.
- Provide temporary redirects only for high-value legacy URLs; do not reproduce broken page structures.
- Require re-authentication/session rotation when security model changes.
- Show users a clear workspace/resource reconciliation report after migration.
- Keep old web read-only for support during the rollback window.
- Validate web permissions against the same migration mapping used by desktop and MCP.

### 17.6 Web acceptance gates

- Account creation/login/session revoke works on supported browsers.
- Workspace/member/team/resource ACL matrix passes allow/deny tests.
- Profile share/revoke and lock state agree with desktop.
- Admin actions are audited and cannot bypass server policy.
- No route renders fake records or a dead primary action.
- Every page has loading, empty, error, forbidden, offline/degraded, and success states where relevant.
- Public pages pass responsive, SEO, accessibility, and performance checks.
- Operational pages pass keyboard, table, filter, dialog, bulk-action, and mobile/desktop checks.
- Screenshot review finds no placeholder/stub/AI-slop patterns.
- Web rollback does not alter migrated source data.

## 18. Full E2E execution and multi-agent orchestration

The target is now a full replacement program, not a prototype. Multi-agent execution is permitted, but concurrency is controlled by architecture boundaries and integration capacity.

### 18.1 Full E2E definition

A release candidate must prove this complete journey with real services and real persisted data:

1. User signs up/signs in on web.
2. User creates or joins an organization/workspace.
3. Admin creates teams, invites members, and assigns capabilities.
4. Desktop authenticates through the approved device/session flow.
5. User creates a profile with runtime/template/defaults.
6. User pastes a mixed proxy batch; parser reports valid/invalid/duplicate records.
7. User assigns a proxy and launches the profile.
8. Browser preserves cookies, local storage, extensions, and profile state after relaunch.
9. User shares the profile with another member.
10. Second member sees the resource and its capabilities.
11. Writer lease prevents conflicting simultaneous mutation.
12. Profile metadata and allowed browser data sync between Windows and macOS.
13. Offline edits queue and reconcile after reconnect.
14. REST and MCP execute approved actions through the same ACL/audit path.
15. Admin sees users, workspaces, resources, jobs, locks, incidents, and audit events.
16. Backup/restore recovers the profile and control metadata.
17. A BugLogin 1.x fixture migrates through inspect, dry-run, import, verify, and rollback.
18. Revocation removes access and invalidates relevant sessions/tokens.
19. Update/restart preserves data and releases runtime/proxy/sync resources.
20. Diagnostics expose actionable evidence without credentials.

This journey runs against all supported runtime adapters and both operating systems.

### 18.2 E2E environment matrix

Required environments:

- Windows physical/VM runner.
- macOS physical/VM runner for every supported architecture.
- Rust control server.
- PostgreSQL staging database.
- Redis staging instance for ephemeral locks/presence/jobs only.
- S3-compatible object storage staging namespace.
- Two user identities, two teams, two workspaces.
- Valid/invalid/slow proxy fixtures.
- Legacy BugLogin migration fixtures.
- Wayfern and every supported legacy runtime binary/version.
- Isolated secrets and signing/test credentials.

No E2E test may use production credentials or mutate production data.

### 18.3 Fifty-agent allocation

Use up to 50 agent assignments across controlled waves. Do not run 50 overlapping write agents.

| Group | Slots | Ownership |
|---|---:|---|
| Architecture/contracts | 4 | domain model, command registry, schemas, ADRs |
| Rust browser/core | 8 | profile, runtime adapters, proxy, VPN, cookies, extensions, lifecycle, diagnostics |
| Control plane/security | 8 | identity, organization, workspace, team, member, resource, ACL, audit |
| Sync/migration/storage | 7 | SQLite/Postgres, object storage, manifests/chunks, encryption, migration, backup |
| Desktop frontend | 5 | shell, profiles, create flow, proxy/resources, jobs/settings |
| Web frontend | 6 | public, auth/account, workspace, members/resources, admin, design system |
| API/MCP/contracts | 4 | REST/OpenAPI, MCP adapters, generated clients, compatibility tests |
| Platform/release | 3 | Windows, macOS, CI/signing/update |
| QA/security/performance | 5 | E2E, visual, security, load, migration/recovery review |
| **Total** | **50** | |

Agent slots are work packets. Actual parallelism is limited to disjoint write scopes and available integration/test capacity.

### 18.4 Agent wave model

#### Wave 0 — architecture lock

- domain entities and IDs;
- permission/resource model;
- command registry contract;
- API/OpenAPI/MCP conventions;
- runtime adapter contract;
- storage/sync/migration formats;
- design system foundations;
- E2E scenario IDs.

No broad implementation begins until Wave 0 contracts are approved.

#### Wave 1 — independent foundations

Parallel write scopes:

- Rust workspace and shared domain crate;
- desktop shell/upstream integration;
- control server skeleton;
- generated TypeScript client pipeline;
- design tokens/primitives;
- test environments and fixtures;
- migration inspector;
- CI runners.

#### Wave 2 — vertical slice

One end-to-end capability path:

```text
account -> workspace -> create profile -> parse proxy -> launch -> stop -> MCP -> audit
```

All interfaces and permission paths must agree before domain breadth expands.

#### Wave 3 — domain breadth

Parallel disjoint domains:

- runtimes;
- profiles/cookies/extensions;
- proxy/VPN;
- organization/workspace/team/member;
- sharing/leases;
- sync/storage;
- web/account/admin;
- migration/backup/restore.

#### Wave 4 — cross-domain E2E

Agents stop adding breadth. QA agents run complete journeys, failure paths, role matrices, Windows/macOS, sync interruption, migration, rollback, and UI review.

#### Wave 5 — hardening/release

Security, performance, updater/signing, compatibility, support diagnostics, documentation, residual-risk review, production rehearsal.

### 18.5 Ownership rules

- One owner per crate/module/screen family.
- Write sets are disjoint within a wave.
- Shared contracts are changed only by contract owners after impact review.
- Agents must not create parallel DTO/domain/permission implementations.
- Agents must not revert or overwrite another agent’s work.
- Every agent receives exact files, inputs, expected outputs, tests, and stop conditions.
- Read-only reviewers may work in parallel with writers; reviewers do not silently patch unrelated code.
- Integration owner merges/contracts conflicts and runs gates.

### 18.6 Mandatory agent output

Each agent returns:

- owned scope and files changed;
- requirement/capability IDs covered;
- contracts used/changed;
- tests added and commands run;
- screenshots/evidence where UI is touched;
- unsupported or deferred cases;
- security/data/migration impact;
- known residual risk.

“Implemented” without evidence is rejected.

### 18.7 Integration gates per wave

- clean compile/typecheck;
- targeted unit/integration tests;
- generated API/MCP contracts remain synchronized;
- no duplicate domain types/permissions;
- no stubs/mock production paths;
- visual review for touched screens;
- cross-agent diff review;
- E2E vertical slice still passes;
- source/migration fixtures remain immutable;
- Windows/macOS smoke where platform behavior changed.

No next wave begins with critical gate failures.

### 18.8 Why not fifty simultaneous writers

Fifty simultaneous writers would maximize merge conflicts, duplicated abstractions, schema drift, inconsistent UI, and false test confidence. The productive model is many agents, limited concurrent writers, many parallel reviewers/testers, and frequent integration. Agent count does not replace architecture or real Windows/macOS execution.

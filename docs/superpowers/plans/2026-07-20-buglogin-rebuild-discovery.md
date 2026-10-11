# BugLogin Rebuild Discovery Plan

Status: Gate 0 controlled implementation. Production implementation remains blocked until migration, UI approval, and cross-platform release evidence pass.

## Phase A — baseline audit

- [x] Locate legacy repository at `E:\bug-login`.
- [x] Clone legacy reference into `.tmp/buglogin-legacy`.
- [x] Clone DonutBrowser upstream into `.tmp/donutbrowser-upstream`.
- [x] Add `.tmp/` to root `.gitignore`.
- [x] Record source snapshots and repository health.
- [ ] Inventory actual user workflows with screenshots or recorded acceptance examples.
- [ ] Mark legacy features as active, dormant, or unknown with owner evidence.

## Phase B — product decisions

- [ ] Select one primary persona and top three jobs-to-be-done.
- [ ] Freeze v1 keep/drop/later matrix.
- [ ] Decide supported OS and runtime/version policy.
- [x] Decide deployment split: `local` is standalone/self-hosted; `production` is hosted HTTPS API only.
- [ ] Decide VPN, DNS, MCP, and API launch scope.
- [ ] Complete AGPL, attribution, trademark, branding, and acceptable-use review.

## Phase C — contracts before code

- [ ] Freeze profile/group/proxy/VPN/extension/cookie/sync schemas.
- [ ] Freeze lifecycle state machines and error taxonomy.
- [x] Freeze Rust command authority and deployment-specific transport: local authority for `local`; hosted HTTPS API for `production`.
- [ ] Define runtime/update trust root and checksum/signature policy.
- [ ] Define migration source versions and destination compatibility rules.
- [ ] Build anonymized migration fixtures and expected reports.
- [x] Lock Donut-compatible UI authority and remove conflicting static prototype references.

## Phase D — architecture spike

- [ ] Build a disposable spike, not production code, for Wayfern launch/stop/crash cleanup.
- [ ] Measure profile startup, memory, disk, concurrent launches, and shutdown reliability.
- [ ] Verify proxy protocols, credentials, location/fingerprint behavior, and failure recovery.
- [ ] Verify extension/cookie persistence across relaunch.
- [ ] Verify default-browser/deep-link behavior per OS.
- [ ] Verify sync of one profile and one config entity with interruption/retry.

## Phase E — implementation order

1. Upstream baseline and brand boundary.
2. Profile/runtime lifecycle hardening.
3. Proxy/network and groups.
4. Import/export and migration.
5. Extensions/cookies/API/diagnostics.
6. Optional VPN/DNS/MCP.
7. Self-host sync and E2E encryption.
8. Separate control plane, only if approved.

## Phase F — release gates

- [ ] Fresh install works on every supported OS.
- [ ] Upgrade preserves profiles and settings.
- [ ] Migration is dry-run first, copy-only, idempotent, rollback-capable.
- [ ] Runtime/update artifacts verify before install.
- [ ] API handlers and OpenAPI remain synchronized.
- [ ] No secret leakage in logs/errors/diagnostics.
- [ ] Crash/kill/update leaves no orphan browser/proxy/VPN process.
- [ ] Sync conflict and interrupted transfer recover without data loss.
- [ ] Manual smoke matrix passes.
- [ ] Security, licensing, and acceptable-use reviews sign off.
- [ ] Every P0 screen has an approved reference package with screenshot, interaction, accessibility, and responsive evidence.

## Stop conditions

Stop implementation and return to decisions when:

- a feature needs a second runtime/engine;
- a cloud entitlement is required for offline profile access;
- migration cannot prove source immutability;
- a local API needs remote exposure without a threat model;
- a UI request adds a portal/admin/billing concern to the desktop core;
- upstream changes alter runtime, license, or data contracts.

## Expanded workstreams

- [ ] Split desktop core, control plane, data plane, and automation plane.
- [ ] Define Rust modular-monolith workspace; remove NestJS unless deployment constraints require it.
- [ ] Specify proxy parser grammar, normalization, duplicate policy, and benchmark targets.
- [ ] Define organization/workspace/team/member/resource/grant/role/capability model.
- [ ] Define profile share, clone, export, lease, revocation, and audit semantics.
- [ ] Define legacy-browser and Wayfern adapter capabilities/version matrix.
- [ ] Define entitlement policy: BugLogin core open; safety controls retained.
- [ ] Define universal command registry shared by Tauri, REST, MCP, and UI.
- [ ] Define sync storage, chunking, encryption, resumability, conflict, tombstone, and garbage collection.
- [ ] Define profile creation templates, progressive disclosure, and bulk workflow.
- [ ] Add backup/restore, trash, job center, diagnostics, quotas, audit, token revocation to discovery backlog.

## New hard gates

- [ ] MCP tool exists for every approved business capability before UI completion.
- [ ] Every MCP tool uses the same ACL and audit path as UI/API.
- [ ] Every shareable resource has owner, scope, grant, revocation, and concurrency semantics.
- [ ] Every runtime declares capabilities; no fake parity between legacy engines and Wayfern.
- [ ] Sync passes interruption, conflict, revoke, checksum, tombstone, and crash-resume tests.
- [ ] Proxy parser passes large-batch, delimiter-in-password, IPv6, duplicate, and partial-failure fixtures.
- [ ] Removing PRO gates does not remove data-loss/security locks.
- [ ] No new feature lands without schema, permission, API, MCP, audit, UI, test, and migration impact review.

## Recommended build order after approval

1. Proxy parser + profile-create vertical slice.
2. Runtime adapter contract with Wayfern + one legacy runtime.
3. Shared resource/ACL/member/workspace slice.
4. Profile share + lease + audit.
5. Shared command registry + MCP/REST/Tauri adapters.
6. Sync metadata/chunk vertical slice.
7. Legacy migration.
8. Remaining admin/HR/resource/browser features.
9. Performance/security/release hardening.

Do not start with the full admin portal, full sync service, or full UI rewrite. Start with the vertical slices that prove the architecture.

## Quality-control workstream

- [ ] Build capability registry from legacy evidence and user requirements.
- [ ] Assign every capability an explicit keep/replace/later/drop decision.
- [ ] Create `DESIGN.md` before production UI implementation.
- [ ] Approve reference flows/screens for desktop and web.
- [ ] Define shared UI primitives and state contracts.
- [ ] Ban release stubs, fake data, empty handlers, and silent disabled actions.
- [ ] Require loading/empty/error/permission/offline/partial/locked states per screen.
- [ ] Require Tauri/API/MCP/ACL/audit/test coverage per capability.
- [ ] Capture fixed-viewport screenshots for every major screen.
- [ ] Run interactive browser QA for desktop webviews and web admin.
- [ ] Run visual review before moving to the next screen.
- [ ] Maintain OS/runtime/role/network/data test matrix.
- [ ] Freeze features before release-candidate QA.
- [ ] Publish acceptance evidence and residual-risk report for each checkpoint.

## Definition of done

A capability is done only when its registry row is complete, real data works, all required states exist, permissions are enforced server-side, REST/Tauri/MCP contracts agree, audit is present, tests pass, screenshots pass review, and migration/upgrade impact is documented.

A release is done only when no critical/high data-loss, permission, runtime, sync, or blocking UX issue remains.

## Production migration workstream

- [ ] Inventory local, control-plane, object-store, and secret data separately.
- [ ] Implement read-only `inspect` and immutable `snapshot` first.
- [ ] Define neutral export package and version it.
- [ ] Build old-to-new entity mapping tables with deterministic IDs.
- [ ] Add per-profile classification: direct, adapter, partial, unsupported.
- [ ] Build encrypted migration reports with secret redaction.
- [ ] Rehearse on cloned production data twice.
- [ ] Verify counts, hashes, runtime launch, cookies, extensions, proxy, ACL, sharing, sync, REST, and MCP.
- [ ] Use a short write-freeze cutover for first production migration.
- [ ] Keep old system/data immutable and recoverable.
- [ ] Test rollback before enabling public writes.
- [ ] Retain 2.0 divergence separately if rollback happens.
- [ ] Archive signed mapping/checksum/residual-risk report.

## Migration stop conditions

Stop cutover when:

- any source data is mutated unexpectedly;
- any profile lacks a deterministic result;
- credentials appear in logs/reports;
- object hash verification fails;
- permission/share behavior differs without approval;
- old clients can still write after freeze;
- rollback cannot restore the old known-good path;
- browser launch/cookie persistence fails for a critical profile cohort.

## Full web replacement workstream

- [ ] Inventory legacy web jobs, data contracts, permissions, and broken flows; ignore old UI structure.
- [ ] Define public site, identity/account, workspace operations, and platform admin boundaries.
- [ ] Define web IA and stable route proposal.
- [ ] Define account/session/device/token security flows.
- [ ] Define workspace/team/member/resource/grant/audit screens.
- [ ] Define platform admin operations and support boundaries.
- [ ] Create separate public/utility/operations/admin design modes in `DESIGN.md`.
- [ ] Prototype six high-risk web flows before broad implementation.
- [ ] Generate web API client/types from OpenAPI.
- [ ] Enforce server-side ACL on every web mutation.
- [ ] Add web states: loading, empty, error, forbidden, offline, partial, success.
- [ ] Run screenshot and interaction QA across desktop/tablet/mobile.
- [ ] Migrate web data through the production mapping/cutover plan; do not migrate old UI.

## Web stop conditions

Stop web implementation when:

- a page has no confirmed user job;
- a mutation lacks server-side ACL/audit/MCP mapping;
- UI relies on mock records outside tests;
- primary actions are stubs or hidden behind unfinished routes;
- the design system is not approved;
- screenshot/interaction QA has not passed the reference flows.

## Multi-agent full-E2E execution

- [ ] Assign capability IDs and exact ownership before spawning agents.
- [ ] Reserve 50 work packets across architecture, core, control, sync, UI, API/MCP, platform, and QA.
- [ ] Limit simultaneous writers to disjoint modules and current integration capacity.
- [ ] Run architecture-lock Wave 0 before broad code generation.
- [ ] Build one full vertical slice in Wave 2.
- [ ] Expand domains only after vertical E2E passes.
- [ ] Freeze breadth during cross-domain E2E.
- [ ] Require every agent to return tests/evidence/residual risks.
- [ ] Use independent read-only review agents for security, design, migration, and contract drift.
- [ ] Run Windows/macOS smoke after every platform/runtime wave.
- [ ] Run full E2E account/workspace/profile/proxy/share/sync/MCP/admin/migration journey before release.

## Agent orchestration stop conditions

Stop the wave when:

- two agents edit the same authority module without coordination;
- domain/schema/permission contracts diverge;
- a shared type is duplicated;
- integration gates fail critically;
- UI agents implement mock/stub flows;
- tests pass only in isolation while the vertical E2E fails;
- Windows/macOS evidence is unavailable for platform-sensitive changes.

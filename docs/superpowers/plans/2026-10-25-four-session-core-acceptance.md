# BugLogin: four sessions, four-hour execution plan

Spec: ../specs/2026-10-25-four-session-core-acceptance-design.md
Coordinator: root. Workers: three Luna sessions. Execution authorized by user.
Start the four-hour clock when the coordinator dispatches all three packets.

Tracking: epic `buglogin-qmv`; active tasks `buglogin-qmv.1` (root), `.2`
(luna_roles_resources), `.3` (luna_runtime_lifecycle), `.4` (luna_sync_locks).
The three workers were launched as gpt-6-luna/high with fresh task context.
Beads Dolt server restored on local port 49272; task persistence checked.

### Integration assignments updated during execution

- S1 exclusively owns canonical/generated integration for the new
  organization.member.workspace.assign command and legacy_compat.rs count/settings
  fixes. Root completed its lib.rs/remote_api.rs native wiring.
- S2 exclusively owns opt-in app_instance.rs isolation and necessary bootstrap,
  preferences, local-feature directory and vault-namespace edits, including
  root's runtime_config/lib bootstrap scope. Preserve new native command wiring.
- S3 owns the new controlled browser fixture and persistent disposable local QA
  API launcher under tests/e2e/support, plus its diagnostics/sync helper calls.
- Root alone drives installed/debug app GUI, commits and deploys/releases.
- Manual GUI launcher: `pwsh -NoProfile -File scripts/e2e/start-two-client-qa.ps1`.
  The tool automatic approval review rejected root's GUI Start-Process with
  instance-specific environment (reason returned only as blocked by policy).
  Do not bypass that rejection through another process-launch mechanism.
  Local QA API is separately running; root will verify its live handle before use.
- First local checks: assignment API/service boundaries, registry8/8 and backend
  unavailable-count1/1 pass; app-instance path/config tests3/3 pass. These are
  intermediate regression checks; A01-A15 remain open until directly executed.

## Starting state (inspect before relying on it)

- Workspace F:/Herd/buglogin; branch actions-sync-main; HEAD a250ec7b; tag v3.0.8.
- 67 modified/untracked entries from earlier work. Preserve all; no reset/clean.
- Local Windows 3.0.8 installer opens BugLogin. Only one authenticated client was
  inspected. Two-client BugLogin acceptance has not run.
- User reports profile access denied and Wayfern identity creation failure.
- Profile list was observed changing from 1 to 0; cause remains to reproduce.
- Public API verifier found three absent operations: organization.account.create,
  organization.members.list, organization.member.status.update.
- Local release tests (29), contract tests (36), desktop self-tests/typecheck pass.
  Strict workspace Clippy fails. These checks do not close live feature cases.
- Previous Donut app tests are excluded. Donut was closed; do not reopen it.
- Existing plans cover organization membership, Windows process isolation,
  cross-workspace transfer and Tauri updater signature verification.

## Working protocol / exclusive writers

All sessions read AGENTS.md and docs/agents/work-packet-ownership.md first.
For UI also read docs/design/ui-authority.md and the pinned reference.
The session scopes below are temporary explicit integration assignments.
Do not edit another scope. Send a precise patch request to its owner instead.
Do not git add/commit/push/deploy or change versions from worker sessions.
Only root drives GUI, manages test accounts, integrates contracts, commits and
releases. Workers must not click shared desktop windows or modify user app data.
Never print secrets. Do not create separate tracking systems; use bd when healthy.
Only root updates this master plan. Workers report progress through messages.
Root serializes heavy Cargo builds because target output is shared.

## Timebox

| Time | Root | S1 roles/resources | S2 lifecycle/runtime | S3 sync/locks |
|---|---|---|---|---|
| 0-15m | establish A/B environment, check release/API tooling | reproduce ACL/resource failures | reproduce identity/start/stop failure | trace two-client sync/lease path |
| 15-75m | implement session isolation, integrate requested wiring | minimal UI+API fixes | native runtime/lifecycle fixes | manifest/key/lease fixes |
| 75-120m | install first candidate; run core A/B loop | run role/resource regression | verify real Wayfern path | verify sync/lock regression |
| 120-180m | commit/build next immutable candidate; deploy API via documented path | close role matrix/UI errors | close multi-profile/restart cases | close interruption/revocation cases |
| 180-240m | installed-app retest, updater roundtrip, publish passing release | retest assigned failures | retest assigned failures | retest assigned failures |

First candidate target: by 120m, earlier if core fixes are ready. Do not wait for
unrelated breadth. If a dependency fails, diagnose it immediately and keep the
other sessions productive. Do not call a failing candidate stable.

## S0 ROOT: integration, two clients, QA and release

Owned source: apps/desktop/src-tauri/src/{lib.rs,remote_api.rs,runtime_config.rs,
auth_vault.rs,device_identity.rs,device_key_vault.rs}; release workflows/scripts;
version files; canonical contracts and generated registries; e2e harness/docs.
Any unlisted integration file is assigned explicitly before editing.

Tasks:
- [ ] Inspect live processes and interrupted command handles; do not restart a live job blindly.
- [ ] Restore bd service or report exact failure; docs here are user-requested plan artifacts.
- [ ] Provide two isolated BugLogin clients: device IDs/keys, vault sessions,
      WebView stores, profile caches and preferences all separate. Changing only
      BUGLOGIN_PROFILE_CACHE_ROOT is insufficient; AuthVault keys use device ID.
- [ ] Provision QA users A/B, org O1/O2, workspaces W1/W2 and disposable profiles
      through existing admin flows. Use A without Platform Admin for owner tests.
- [ ] Execute core acceptance below via real installed app commands/controls.
- [ ] Integrate worker wiring/contract requests; generate, never hand-edit generated output.
- [ ] Fix strict CI failures needed for the release; coordinate source owners.
- [ ] Review dirty changes, commit/push authorized work, choose next unused
      version (3.0.9 only if still unused), build Windows/macOS immutable candidate.
- [ ] Deploy API through scripts/release/deploy-api-systemd.ps1 after snapshot,
      backfill rehearsal/verification and rollback preparation; verify public API.
- [ ] Install candidate, run updater from prior version, verify preserved data.
- [ ] Promote passing candidate through release.yml/promote-stable.yml; read
      public feed/artifact bytes back. Continue release/fix cycles for remaining failures.

## S1 LUNA: accounts, roles, resources and corresponding UI

Owned source: crates/domain/src/{account,organization,member,permissions,team,
workspace,resource,grant}/**; relevant domain tests; crates/api/src/services/
{members,resources,workspaces,teams}/**, organization_members.rs; API HTTP
mod.rs/control.rs and MCP mod.rs (request sync-related changes from S3 first);
src/components/{org-management-view,organization-members-page,workspace-detail-page,
resource-inventory-panel,org-member-access,org-workspace-route,scoped-account-creation-page,
account-management-panel,platform-admin-view}*; apps/desktop/src/bridge* and
organization-account-input*/org-workspace-route*; src/hooks/use-profile-events.ts
and src/app/desktop-home.tsx only for profile listing/workspace context defects.

Tasks:
- [ ] Trace reported profile denial and list 1->0 through account/workspace,
      membership, binding/grant, list filtering and UI request ordering.
- [ ] Repair incorrect allowed-action denials; preserve genuine scope denials.
- [ ] Finish existing org membership/Workspace Access UI and its real API flow.
- [ ] Cover account/member create/list/update/status, invites, role changes,
      resource CRUD/assignment/share/revoke; include persistence/reload.
- [ ] Test Platform Admin, Org Owner and workspace Owner/Admin/Lead/Operator/Member.
      Use existing service policy; no invented role semantics or blanket allow.
- [ ] Test cross-org/workspace IDs, explicit deny, suspended/revoked/invited user,
      lower-role escalation and last-owner protection. API must reject bypasses.
- [ ] Fix UI names, loading/error/retry, form state and responsive overflow on
      these flows. IDs must not be ordinary person/workspace labels.
- [ ] Send root exact native/remote/contract changes required; return test names
      and expected GUI actions. No worker deploy or global version bump.

## S2 LUNA: Wayfern identity, browser download, launch/stop

Owned source: apps/desktop/src-tauri/src/{wayfern_fingerprint.rs,runtime_install.rs,
profile_lifecycle.rs,profile_runtime.rs}; lifecycle/runtime modules actually
referenced by them under crates/browser/src/{runtime,lifecycle,process,platform}/**;
related browser tests and apps/desktop/src-tauri/tests/runtime_lifecycle_native.rs.
Request root changes to lib.rs/runtime_config.rs; S3 owns desktop_state.rs.

Tasks:
- [ ] Trace exact Wayfern identity failure. cdp_command currently erases errors
      into (); retain safe error classification and repair actual method/params,
      target/version or authorized entitlement integration. Never fake identity.
- [ ] Confirm real selected binary/version and isolated user-data-dir.
- [ ] Download/install/retry browser versions with catalog integrity checks.
- [ ] Create->launch->stop->relaunch, two profiles A/B, stop A leaves B usable.
- [ ] Verify root/child cleanup by owned PID/Job Object, not broad name killing.
- [ ] Test app shutdown/restart, stale state and profile locks. Keep real
      headful acceptance; separately diagnose failing headless paths.
- [ ] Send S3/root precise launch/orchestration changes and operational error codes.

## S3 LUNA: hosted profile sync, browser data and team leases

Owned source: apps/desktop/src-tauri/src/{desktop_state.rs,profile_projection.rs,
sync_features.rs,diagnostics.rs}; crates/sync/**; crates/storage/src/{remote,backup}/**
and related tests; crates/api/src/services/{profile_sync.rs,profile_hosted.rs,leases/**}.
Request HTTP/MCP wiring from S1 and remote_api/lib/contracts from root.

Tasks:
- [ ] Trace stop->final snapshot->upload->manifest commit->lease release;
      download->verify->decrypt->projection->launch; fix missing links.
- [ ] Test same-account/different-device first, then A/B with explicit share.
- [ ] Preserve controlled browser data/tabs across A->B->A and process restarts.
- [ ] Enforce one writer for simultaneous opens; heartbeat/TTL/fencing must
      reject stale writes, including after partition/crash/revocation.
- [ ] Test interrupted transfer, retry, lost commit response and checksum failure.
      Do not publish partial manifests or report synced before durable commit.
- [ ] Verify envelope recipients/device enrollment and immediate revoke/suspend.
- [ ] Send sanitized operational errors via durable outbox; retain retry on
      failure. Root verifies actual production receipt, not only an empty queue.
- [ ] Distinguish group moves from canonical workspace transfer; coordinate any
      exposed transfer work with S1 using the existing transfer spec.

## Core direct acceptance (root runs; workers fix their owned failure)

| ID | Case | Required outcome | Owner | State |
|---|---|---|---|---|
| A01 | two BugLogin clients, two accounts | independent login/logout/workspace/device | root | pending |
| A02 | owner/admin/resource UI+API | create/list/edit/assign/share/revoke survives reload | S1 | pending |
| A03 | every workspace role and platform/org authority | allowed succeeds; forbidden API calls denied | S1 | pending |
| A04 | profile list/denied bug | correct selected scope; no disappearing authorized record | S1 | pending |
| A05 | browser version download/retry | verified installed version can launch | S2 | pending |
| A06 | Wayfern identity/open/stop/reopen | stable identity, real browser, clean owned-process stop | S2 | pending |
| A07 | two profiles local isolation | stopping one leaves other alive and usable | S2 | pending |
| A08 | A->B->A browser data/tabs | persistent fixture data restored and updated correctly | S3 | pending |
| A09 | simultaneous team open | one lease/writer; clear holder UI/API denial | S3 | pending |
| A10 | stop/final sync/release->other client open | latest committed generation used | S3 | pending |
| A11 | crash/network/retry/stale token | recovery without stale overwrite/permanent lock | S3 | pending |
| A12 | suspend/revoke/lower-role/cross-tenant | immediate server-side denial, no data leakage | S1/S3 | pending |
| A13 | operational diagnostic | root finds sanitized receipt on production server | S3/root | pending |
| A14 | new immutable installed candidate | Windows/macOS install/login/core smoke passes | root | pending |
| A15 | prior release->new release updater | feed/signature/install/restart/data retention passes | root | pending |

## Reporting and checks

At 30-minute checkpoints each worker sends: fixed defect, files changed, targeted
checks, remaining failing acceptance ID, exact integration request. Send urgent
contract conflicts immediately. No long status essays or repeated broad reruns.
Each failure record: case ID, client/version/role, action, actual error code,
expected outcome, patch owner, retest outcome, release containing the fix.

Run appropriate existing targeted tests, then root integration checks:
pnpm --filter @buglogin/desktop test; pnpm --filter @buglogin/desktop typecheck;
pnpm test:contract; pnpm check:api; pnpm release:verify;
cargo fmt --all -- --check; relevant cargo tests and required CI checks.
Do not advertise planned pnpm e2e project commands as runnable: the current
tests/e2e/package.json has smoke:core/smoke:control/qa:ui, not all manifest aliases.
The binary smoke script checks process liveness only; it does not satisfy A01-A15.
pnpm test:sync-e2e exercises legacy donut-sync/MinIO; also run Rust hosted sync and
the actual two-client BugLogin flow. Exclude all Donut app results.

## Current priority and one-pass sweep (2026-10-25)

User narrowed the next release to profile create/download/open/stop/reopen,
browser-data/tabs sync, and opening the committed profile from a second client.
Resource/role management follows in the subsequent release. No production
release or deployment has been performed for these uncommitted changes.

- Direct BugLogin UI: two isolated 3.0.9 clients login as Owner/Member and share
  the workspace; profile create/list works. Launch reproduced Wayfern identity
  failure. After an encrypted snapshot commits, Owner profile reads fail with
  `profile_forbidden`; this also explains the disappearing list entry.
- Proven sync root cause: first V2 encrypted manifest had no current-device
  owner key envelope. Initial enrollment is now required and stored with the
  manifest mutation. Focused `initial_encrypted_manifest_enrolls_only_the_authenticated_owner_device`
  test passes. Existing V2 profiles lacking an envelope still need authorized
  key recovery; do not delete data or bypass E2EE to make them visible.
- SQLite concurrent upload locks previously fell back to exclusive locks,
  producing false `sync_object_cleanup_in_progress`. Shared SQLite locks now
  allow writers while excluding cleanup; focused regression passes.
- Legacy UI launch now calls the exact-version verified runtime installer and
  submits failed launches to the durable diagnostics outbox. Local API receipts
  for real launch errors were observed. CDP classification is whitelisted in
  diagnostic context; untrusted fields are excluded (regression passes).
- API-only sweep passes profile create/update/delete/share, expected member
  create denial, device-key registration, lease exclusion/release/reacquire, and
  encrypted blob/manifest commit/readback with checksum/decryption match.
  These results do not satisfy direct A/B browser-data acceptance.
- Native isolated Wayfern 154/155 identity probes pass; signed installed version
  records and executable hashes match. No wrong-binary selection was proven.
- Workspace suspended-member restoration gap is patched for a later release;
  role ceiling tests were added. The apparent Owner escalation in the sweep was
  the fixture's intentional PlatformAdmin authority, not a proven exploit.
- Full API lib run: 278/281 passed; enrollment fixture and role-test correlation
  were corrected afterward. A concurrent credential-capacity test failure still
  requires an isolated rerun. Generated contracts: 36/36 and check:api pass.
- Production API lacks four current organization operations; stable updater
  feed is 3.0.7. Official candidate requires source synchronization and signed
  Windows/macOS evidence under the documented release process.

### Direct two-account browser sweep, 2026-10-26

- Fresh isolated QA run on the rebuilt 3.0.9 frontend: Owner creates and launches
  `Sync release check`; first encrypted snapshot commits as generation 2 and the
  Owner shares Operate with the Member, including the confirmed device envelope.
- A to B: Member sees the profile, launches it, automatically restores both
  saved fixture tabs, and reads cookie, localStorage and IndexedDB as
  `written-on-device-a` (all three PASS).
- B changes and saves `written-on-device-b`; all three local readbacks PASS.
  B stops in the app and commits manifest generation 3 with 166 objects.
- A relaunches and downloads generation 3. The fixture reads
  `written-on-device-b` from cookie, localStorage and IndexedDB. Its displayed
  FAIL compares against its default expected marker `written-on-device-a`; the
  actual marker matches B's saved value. This confirms A to B to A data sync.
- Profile opens and stops were exercised in both isolated app instances. Tab
  restoration succeeded automatically in the latest run. The Windows shutdown
  patch now requests a graceful main-window close before fallback termination.
- Reverse-sync root cause: Operate shares grant Read/Execute, while snapshot
  writes required profile.update. Sync-only authorization now accepts the
  encrypted device-authorized Execute path, preserving explicit denies, token
  restrictions and key-rotation guards. Focused regression passes, including
  read-only denial and metadata-update denial. Updated local API is ready;
  B's pending data now produces successful blob uploads during launch recovery.
- Organization assignment/owner-ceiling integration test now passes after
  aligning implicit organization ownership with member management authority.
  MCP account/membership atomic test passes in isolation. No release claim yet.
- Full API library suite: 283/283 passed. Desktop UI tests and TypeScript
  typecheck passed. Release workflow/signature checks passed 24/24. This remains
  local candidate evidence; the official Windows/macOS signed release gates and
  production S3 promotion have not run.
- Landing website is part of release acceptance: update/verify its latest
  download links alongside the S3 updater feed after signed artifacts are ready.

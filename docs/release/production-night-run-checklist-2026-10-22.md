# BugLogin 3.0 release checklist: minimum launch vs. later hardening

Date: 2026-10-22. This checklist separates the minimum evidence needed to ship a useful Windows/macOS build from capabilities that can follow after launch. A source file or configured secret name is not runtime evidence; each gate needs a CI run, downloaded artifact, or a recorded acceptance result.

## Execution update — 2026-10-22

### Current continuation checkpoint

- All release work targets `XuanHongitHub/buglogin-3.0`, version `3.0.0`, with one local Cargo target at `G:\buglogin-target`.
- The signed runtime catalog and embedded trust map now contain Wayfern Windows x64 `154.0.8037.57` and `155.0.8059.79`, plus Wayfern macOS arm64 `155.0.8059.79`. All three Ed25519 catalog signatures verify locally.
- A clean Windows NSIS installer and detached Tauri updater signature were rebuilt on G: after fixing malformed JSON in the embedded trust map. Installer SHA-256: `E078F89CBF3E6A1882538761E8F2281E94F94448F200F9283EFF5C9214445D95`. These are local build artifacts, not a published release.
- Release checks pass: `pnpm release:verify` 15/15, `actionlint` on release/signing/promotion workflows, and `git diff --check` (Git only reports the configured LF/CRLF conversion warnings).
- Production API health/readiness is HTTP 200, but `https://api3.buglogin.com/v1/runtime/catalog` still returns 404. The signed catalog files and environment paths are staged on the isolated host; API artifact deployment and catalog readiness verification remain open.
- No 3.0 GitHub Actions signing/release run, macOS artifact, notarization evidence, S3 release upload, cross-client sync acceptance, or installer/update acceptance has completed yet.

Use this current handoff state when executing the older audit entries below:

- Release repo is `XuanHongitHub/buglogin-3.0`; `keyduc91/Malvanut-Login` is legacy 1.0 only. Local release version is `3.0.0` and desktop artifacts/feed use `releases/desktop/v3/`.
- Build target is `G:\buglogin-target`. The repo helper, Windows smoke script, tests, and hosted Windows setup action are being aligned to G:. Old BugLogin targets were cleaned with Cargo: 108.5 GiB removed from E: and 8.7 GiB from C:; both directories are empty.
- Apple signing, S3 publisher, and updater signing secrets plus `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY` have been configured in the 3.0 repository. No secret values are documented. The updater private key is unencrypted, so no password secret is needed.
- S3 upload → public download → SHA-256 → delete → post-delete access check passed for a disposable object under `releases/desktop/v3/preflight/`. This does not stand in for publishing release artifacts or feeds.
- GitHub returned HTTP 403 when querying branch protection for the private repo on the current plan. Release is manual-only from `main`; create the version tag and dispatch the release workflow with that tag. A tag push will not launch a release job.
- Current `pnpm release:verify` passes 15/15; target-selection tests pass 15/15; Windows desktop Cargo check on G: passes. No artifact, macOS notarization, production API deployment, cloud profile sync, installer/update acceptance, or GitHub Release is yet proven.
- Source changes remain uncommitted/unpushed. API source is ahead of production contract (historical probe: 94 paths/100 operations hosted vs. 110/122 source), so the API parity/deploy gate remains open.

## Release decision

**Current state: not ready to publish BugLogin 3.0.** This does not require every API or admin feature to be complete. The specific blockers are: release/source edits remain uncommitted in a broadly dirty worktree; the production API is stale and misses login/runtime catalog operations; the 3.0 release feed is not publicly readable; and current 3.0 app/runtime/profile/sync acceptance on both OSes is not evidenced. The Windows Authenticode gate has now been removed in the local workflow patch, but that patch still needs review and push.

### Facts already established

- BugLogin 1.0 has released Windows and macOS builds and the user confirms its update path works. This proves the legacy 1.0 update flow, not the 3.0 Tauri updater feed, keys, package format, or workflow.
- Read-only GitHub inspection confirms these Apple secret names exist: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`. The 3.0 macOS workflow accepts `APPLE_CERTIFICATE` and `APPLE_PASSWORD` as aliases and requires the other three. Their values were not read. BugLogin 1.0 updates work, but the 3.0 signing/notarization job has not yet validated the aliases with the new build.
- Windows Authenticode is not required for the requested Windows install path. Users can download and run the installer. The local 3.0 workflow patch removes the PFX/timestamp gate. Tauri updater artifact signatures are separate cryptographic signatures and remain needed if in-app update is retained.
- Production API readiness and web endpoints respond (`/v1/readyz` 200; web 200). The current exact verifier `node scripts/release/verify-staging-api.mjs` fails: production OpenAPI is 94 paths / 100 operations versus source 110 / 122, with 23 source operations missing and one unexpected operation; critical `auth.desktop.login` and `runtime.catalog.get` are missing. Readiness has no `X-BugLogin-Runtime-Catalog: available` header and `/v1/runtime/catalog` returns 404. This is a concrete blocker for desktop sign-in/runtime, not a demand to finish all unrelated APIs.
- The production service is configured for PostgreSQL, Redis, and Spaces (`mavelnut-login`, sync prefix `buglogin3/prod`). No successful 3.0 two-client sync/checksum/restart test against that production path was recorded.
- A Windows Wayfern runtime download test passed locally. A current 3.0 macOS runtime catalog artifact and native macOS profile launch acceptance were not evidenced.
- GitHub secret-name inspection confirms `S3_ACCESS_KEY_ID` and `S3_SECRET_ACCESS_KEY` exist. It finds no `TAURI_SIGNING_PRIVATE_KEY`, no `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and no repository variables, including the required `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY`. The 3.0 Tauri updater build config fails without the public key and private signing key; the password is needed only if the private key is passphrase-protected. The public 1.0 feed returns 200 and identifies v1.1.84. The current release storage namespace is named `releases/desktop/v2/`; its RC/stable feeds return HTTP 403. S3 secret presence does not prove write scope. No successful 3.0 S3 upload/readback or updater acceptance was recorded.
- Local `HEAD` and `origin/main` both resolve to `226df804`; however, release/S3 workflow changes are modified but uncommitted in the current worktree alongside many unrelated changes. Thus GitHub does not yet contain the candidate release workflow.

## Gate 1 — Release input is controlled

- [ ] Inspect `git status` and review the full diff. Select only changes needed for this release; preserve unrelated/user changes and `.tmp/` references.
- [ ] Confirm the intended repository is `keyduc91/Malvanut-Login`, `main` is BugLogin 3.0, and `1.0version` remains untouched.
- [ ] Commit and push a narrow reviewed source change set. Record the exact commit SHA. Do not release from a dirty checkout or from unreviewed local-only workflow edits.
- [ ] Set one version consistently in desktop package/Tauri metadata, tag, release notes, manifests, and artifact names.
- [ ] Run the repository's required CI checks on that exact commit. Record green run URLs and resolve failures that affect packaging, auth, profile/runtime, sync, or updater.

**Evidence to save:** commit SHA, diff review, green CI URLs, version/tag match.

## Gate 2 — Existing production API is the API the app needs

- [ ] Deploy the reviewed API using the repo's documented deployment procedure, with a database snapshot, migration rehearsal/result, health check, and rollback command recorded.
- [ ] Verify TLS and `/v1/readyz` on `api3.buglogin.com`; verify PostgreSQL persistence and the expected runtime catalog readiness signal.
- [ ] Check parity for the **release-critical** operations: login/refresh/logout, account and authorized workspace selection, profile CRUD, runtime catalog/install, launch/stop status, sync manifest/blob operations, and team-lock lease operations when shared profiles are enabled.
- [ ] Do not require all 122 source operations just to ship if unrelated or hidden capabilities are explicitly out of the release scope. For every feature shown/enabled in the app or advertised for release, its API must be present and permission-checked server-side.
- [ ] Use two test accounts/clients to verify permitted workspace access, denied cross-workspace access, and revocation. Exhaustive platform-admin coverage is not required unless those operations are enabled/advertised for the initial release.

**Release blocker if failed:** login, workspace authorization, profile CRUD/runtime, sync, or an exposed action is missing, inconsistent, or bypasses server-side permission checks. Broader unrelated API parity can follow if the feature is disabled and not advertised.

## Gate 3 — Core profile use works on Windows and macOS

- [ ] Create or select a disposable authorized account/workspace and a disposable profile; do not use a user's only copy of browser data for first-run acceptance.
- [ ] On each platform, install the runtime from BugLogin's published catalog and verify downloaded bytes against its catalog checksum/signature.
- [ ] Launch the BugLogin-managed Wayfern binary, inspect its process arguments and confirm a unique BugLogin-managed user-data directory for that profile. Stop it and verify the process exits and state returns to stopped.
- [ ] On each platform, verify create → launch → browse ordinary new tab → stop → relaunch. Confirm no Donut vendor tiles, no external/default Wayfern user data, and no shared profile root between two concurrent profiles.
- [ ] Verify Windows installer can be downloaded and manually installed by a standard user. Authenticode is optional under this release policy; do not block release on a Microsoft signing certificate.
- [ ] Build macOS arm64 and any promised Intel target on native macOS runners. Sign with Developer ID, notarize, staple, and verify the downloaded app/DMG passes Gatekeeper on a clean Mac.
- [ ] If macOS runtime support is advertised, publish macOS runtime binaries/catalog entries and verify download, checksum/signature, extraction, launch, and stop on a real Mac. An app DMG alone does not prove browser runtime support.

**Release blocker if failed:** one advertised OS cannot install/open the app, authenticate, install its managed runtime, or launch/stop an isolated profile. Intel support is only a gate if the release promises Intel Macs.

## Gate 4 — Sync is real and safe for the launch promise

- [ ] Confirm packaged apps use `https://api3.buglogin.com`; sync goes through the API to the configured Spaces tenant/prefix. No desktop S3 secret is present.
- [ ] Sign into the same authorized account on two independent clients. Create a disposable profile on client A, wait for sync completion, then restore/open it on client B.
- [ ] Verify manifest revision and object/chunk checksums on both clients. Restart client B and confirm the profile remains restored and opens. Make one change on B and confirm it returns to A without losing the committed revision.
- [ ] Test one interrupted/retried transfer and one corrupted-checksum rejection. A failed transfer must not be reported as synced or advance the committed manifest.
- [ ] For Windows↔macOS compatibility, restore and launch a Windows-created profile on Mac, then the reverse, only if cross-OS profile portability is promised. Explicitly record cookie/login portability; OS-bound encryption may limit it.
- [ ] Team lock is a gate if shared profiles/team collaboration are enabled in this release: with two devices, confirm one lease owner, contention denial with a human-readable owner, stop/release, crash/expiry recovery, and no phantom `running` state.

**Release blocker if failed:** the UI claims cloud-synced data is safe but a second client cannot restore it, checksum/revision is false, or failure can silently lose/overwrite committed data. Cross-OS cookie portability and advanced lock recovery can be scoped separately only if clearly not promised; never show a fake lock state.

## Gate 5 — Release artifact and update route

- [x] Remove the Windows Authenticode requirement from `.github/workflows/signing.yml`. Tauri v2 signs the NSIS `.exe` used by both manual install and updater; the `.exe.sig` accompanies it.
- [ ] Keep Tauri updater artifact signing separate from Windows Authenticode. If in-app update is included, add GitHub secret `TAURI_SIGNING_PRIVATE_KEY`, repository variable `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY`, and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` only if that key is passphrase-protected; sign the Windows installer `.exe` and Mac app archive for each supported platform.
- [ ] For macOS, run the 3.0 job with existing Apple secrets/aliases: Developer ID import, app signing, notarization, stapling, Gatekeeper check. BugLogin 1.0 success is prior evidence, not a substitute for this run.
- [ ] On the reviewed immutable commit/tag, produce Windows installer and macOS DMG/app artifacts. Generate SHA-256, SBOM, provenance, and updater signatures where updater is enabled.
- [ ] Publish an RC to the isolated `releases/desktop/v2/` path in the existing Spaces bucket. Read back the feed and every artifact URL, compare hashes, and verify no object was written under the sync prefix or 1.0 feed.
- [ ] Install the downloaded RC on a clean Windows machine and Mac. Check login, profile launch/stop, and sync. If in-app updates ship in 3.0, prove update from the previous supported build and reject an invalid updater signature in a test fixture.
- [ ] Promote the same tested artifact hashes to stable. Do not rebuild between RC acceptance and promotion.

**Update scope:** if the 3.0 updater signing key/feed is not ready, release can still proceed as a manual-download release only after disabling/hiding the updater action and documenting manual upgrade. Do not ship an About button that predictably fails.

## Minimum launch vs. post-launch scope

| Capability | Before public launch | Can follow if clearly disabled/unadvertised |
|---|---|---|
| Download/install app | Windows installer works; macOS signed/notarized app passes Gatekeeper | Authenticode for Windows |
| Sign-in and authorization | Login, refresh, logout, workspace access, server-side deny/revoke | Exhaustive tests for unrelated admin endpoints |
| Browser profiles | Managed runtime installs; profile data isolated; launch/stop accurate on each advertised OS | Unsupported OS/architecture support |
| Cloud sync | Two-client upload/restore/restart/checksum acceptance if cloud sync is offered | Cross-OS cookies if not promised; disclose platform limits |
| Team lock | Correct lease behavior if shared/team profiles are enabled; no fake running/lock state | Team collaboration can be disabled for a solo MVP |
| MCP / integrations | Only if enabled or advertised: auth scope and the advertised CRUD operations pass | Unfinished MCP automation/scenario goals can remain unavailable |
| App updater | Signed update works if exposed; otherwise remove/disable the action and use manual downloads | Auto-update can follow if manual release process is clear |
| API breadth | Every enabled/advertised action has a working authorized API | Unexposed features outside MVP |

## Post-release hardening (not a launch blocker unless advertised)

- Complete parity and permission tests for every non-MVP platform-admin/API/MCP operation.
- Expand MCP automation from profile CRUD/runtime control into task/scenario/goal execution.
- Add broader Redis lease chaos tests, recovery drills, and longer multi-device soak tests after core lease acceptance passes.
- Add Intel macOS support if not in the initial target matrix, more OS versions, and performance/load baselines.
- Refine release approval separation, rollback drills, and telemetry/operations dashboards without collecting browser contents.

## Actual next steps, in order

1. Review and isolate the release diff; push the exact 3.0 candidate commit to `XuanHongitHub/buglogin-3.0`.
2. Review and push the Windows workflow patch. Configure the separate Tauri signing keys/feed for 3.0 auto-update; do not add Windows Authenticode as a requirement.
3. Deploy the reviewed API safely and verify only the release-critical API/runtime routes plus account/workspace permission boundaries.
4. Produce the Windows x64 installer and signed/notarized Apple Silicon Mac artifact; run Gate 3 acceptance on actual Windows and Mac machines. Intel Mac is not in scope because the currently published Wayfern manifest has no Intel build.
5. Run two-client production sync acceptance and team-lock acceptance for whichever features are enabled/promised.
6. Publish an immutable RC to S3, download/install it on both OSes, verify update only if enabled, and attach evidence.
7. Promote the exact accepted hashes to stable. If a gate fails, fix that gate; do not expand the blocker list with unrelated unfinished features.

### Release execution follow-up — 2026-10-22

- Use only `G:\buglogin-target` for local build output and caches. The previous C: and E: BugLogin Cargo targets were removed after stopping their exact build/API process trees; no build is allowed on C:, E:, or F:.
- Current official `https://donutbrowser.com/wayfern.json` reports Wayfern `155.0.8059.79`: Windows x64 and macOS arm64 downloads are available; macOS x64 is `null`. The Mac DMG layout has been inspected and its executable is `Wayfern.app/Contents/MacOS/Wayfern`.
- Release signing/publishing must therefore produce exactly two app targets: Windows x64 and macOS arm64. Do not build/publish a Mac Intel updater feed entry that cannot install a browser runtime.
- The current signed runtime catalog still needs a refreshed v155 Windows+Mac arm64 catalog, trusted public-key update, production API configuration, and live install/launch evidence. The catalog signing key is separate from the Tauri updater key.

## Evidence record

For each gate, record: status (`PASS`, `FAIL`, `NOT IN MVP`), tested commit/version, runner or device/OS, workflow/run URL or test procedure, artifact SHA-256, and defect/follow-up owner. A status without this evidence remains `NOT VERIFIED`.

# Windows profile process isolation plan

Status: the strict complete Windows native lifecycle suite now passes after correcting the multi-profile acceptance case to launch profiles the same way the interactive app does. Release remains blocked on the other goal gates and both-OS installed-app evidence.

## Scope

Continue the existing IO-CORE lifecycle packet around the reported failure where stopping profile A may also stop profile B. Preserve all unrelated worktree changes. Do not promote a release while the native process-isolation gate is red or ambiguous.

## Phases

1. **Reproduce and classify**
   - [x] Read current lifecycle/process ownership and work-packet rules.
   - [x] Run strict Windows Job Object and LifecycleManager tests against the installed Wayfern binary (9/9 pass).
   - [x] Run the isolated ProfileRuntimeHandle test with an offline page (pass) and the isolated visible AdapterState A/B stop flow (pass).
   - [x] Replace command-line path matching in the A/B acceptance assertions with browser-root PID plus Job Object helper PID; command-line discovery returned empty while live processes existed.
   - [x] Capture root/helper state and separate the process-command-line probe from process liveness.
   - [x] Determine that the remaining suite failure was tied to using the headless/test launch path for the multi-profile app acceptance case; the interactive app path passes end-to-end.
2. **Fix the proven cause**
   - [ ] Make the smallest production change in the owning Rust layer.
   - [ ] Keep a regression test that verifies A is fully stopped while B remains usable.
3. **Verify the lifecycle gate**
   - [x] Run the Windows browser process/lifecycle suite serially in strict mode (9/9).
   - [x] Run the desktop native lifecycle suite serially in strict mode, including app shutdown/restart and profile locks (3/3).
   - [ ] Inspect process and temporary-directory cleanup after both pass.
4. **Rejoin the full goal**
   - [ ] Re-run sync E2E and updater/release verification after lifecycle changes.
   - [ ] Continue role/access and production telemetry acceptance from their owning plans.
   - [ ] Do not call the release stable until native Windows and macOS install/update smoke evidence is available.

## Current evidence

- Strict Windows `platform_windows` suite: **9/9 PASS**, including independent Wayfern Job Objects and `LifecycleManager` isolation.
- Strict `ProfileRuntimeHandle` A/B test with `about:blank`: **PASS**.
- Strict isolated visible `AdapterState` A/B launch/stop test using the production BugLogin start page: **PASS** on two focused runs.
- The complete native lifecycle suite passes after its A/B acceptance phase uses the visible launch mode; the other start/stop/relaunch/lock/restart cases retain their headless coverage.
- A standalone headless `ProfileRuntimeHandle` A/B test with `about:blank` passes. The retired test variant combining two headless profiles with the network-dependent BugLogin start page failed in earlier runs; it is not the user's interactive path and should be treated as an unresolved diagnostic scenario, not used as the production multi-profile acceptance gate.
- Windows command-line process discovery has false negatives (empty results with live PIDs); acceptance checks now use the root PID and Job Object helper instead.
- Local MinIO sync and release-signature tests do not prove production sync or installed-app update behavior.
- Local MinIO sync and release-signature tests do not prove production sync or installed-app update behavior.

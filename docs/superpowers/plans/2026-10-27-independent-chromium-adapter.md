# Independent Chromium Adapter Beta

## Goal

Build an opt-in, independently verified browser adapter that can be evaluated
against Wayfern and current profile-management products. Keep Wayfern selected by
default and keep production runtime catalog behavior unchanged.

## Research baseline

See `docs/superpowers/specs/2026-10-27-independent-chromium-adapter-design.md`
for the current comparison and parity bar. Primary references include:

- ShardBrowser launcher/API: <https://github.com/ProxyShard/ShardBrowser>
- ShardBrowser profile lifecycle API:
  <https://docs.proxyshard.com/eng/shardx-launcher-api/profiles>
- CloakBrowser source and binary license:
  <https://github.com/CloakHQ/CloakBrowser>
- CloakBrowser binary terms:
  <https://github.com/CloakHQ/CloakBrowser/blob/main/BINARY-LICENSE.md>
- Donut runtime/profile docs: <https://donutbrowser.com/docs/profiles/>
- Donut proxy/leak protection docs: <https://donutbrowser.com/docs/proxies/>
- Donut self-hosted sync: <https://donutbrowser.com/docs/self-hosting/>
- Donut team locks: <https://donutbrowser.com/team/>
- Camoufox source/architecture/license:
  <https://github.com/daijro/camoufox>
- Camoufox installation/platform notes:
  <https://camoufox.com/python/installation/>
- Fury Chromium candidate:
  <https://github.com/furyteamtop/fury-antidetect-browser>
- Fury persona data contract (Apache-2.0 schema):
  <https://github.com/furyteamtop/fury-antidetect-browser/blob/v0.4.16/shared/persona.schema.json>
- Fury core fingerprint contract and pinned binary patch:
  <https://github.com/furyteamtop/fury-antidetect-browser/blob/v0.4.16/shared-rs/src/fingerprint.rs>
  <https://github.com/furyteamtop/fury-antidetect-browser/blob/v0.4.16/core/patches/0030-canvas-noise.patch>
  <https://github.com/furyteamtop/fury-antidetect-browser/blob/v0.4.16/core/patches/0040-audio-noise.patch>
- Chrome stable release/security currency:
  <https://chromereleases.googleblog.com/2026/10/stable-channel-update-for-desktop_086471744.html>
  <https://chromereleases.googleblog.com/2026/10/stable-channel-update-for-desktop_01668083673.html>
- Chromium process isolation:
  <https://www.chromium.org/developers/design-documents/site-isolation/>
- Chrome extension Manifest V3/service-worker behavior:
  <https://developer.chrome.com/docs/extensions/develop/migrate/what-is-mv3>
- UA and Client Hints are separate surfaces and must agree:
  <https://developer.chrome.com/docs/privacy-security/user-agent-client-hints>
- Chromium's own UA override guidance requires corresponding Client Hints:
  <https://chromium.googlesource.com/chromium/src/+/main/docs/user_agent/README.md>
- W3C fingerprinting threat models and mitigations:
  <https://www.w3.org/TR/fingerprinting-guidance/>
- Brave's fingerprint-protection design and farbling QA matrix:
  <https://github.com/brave/brave-browser/wiki/Fingerprinting-Protections>
  <https://dev-pages.brave.software/fingerprinting/farbling.html>
- Brave's description of deterministic, per-session/per-site randomization:
  <https://brave.com/privacy-updates/4-fingerprinting-defenses-2.0/>
- Mozilla's deployed fingerprinting protections (a privacy-first alternative
  to per-profile spoofing):
  <https://blog.mozilla.org/en/firefox/fingerprinting-protections/>
- Mozilla's current Firefox fingerprinting protection behavior and compatibility
  trade-offs:
  <https://support.mozilla.org/en-US/kb/firefox-protection-against-fingerprinting>
- Camoufox's engine-level surface coverage and explicit Firefox-only boundary:
  <https://camoufox.com/fingerprint/>
- Fury release artifact and exact Chromium version:
  <https://github.com/furyteamtop/fury-antidetect-browser/releases/tag/v0.4.16>
- Fury component license boundaries:
  <https://github.com/furyteamtop/fury-antidetect-browser/blob/main/docs/10-legal-licensing.md>
- Current Chrome stable (Chrome 156.0.8078.47/.48, published Oct 20, 2026):
  <https://chromereleases.googleblog.com/2026/10/stable-channel-update-for-desktop_02018666648.html>
- Existing engine candidate:
  <https://github.com/adryfish/fingerprint-chromium>

## Execution

### Phase 1 — Inventory and contracts

- [x] Confirm isolated beta scope and preserve Wayfern as default.
- [x] Record competitor implementation models, license constraints, and parity
  evidence requirements.
- [x] Verify which beta paths are wired only to an example versus a production
  lifecycle service; do not activate a beta runtime in product selection.
- [x] Define a strict, versioned core-config contract with bounded field validation,
  cross-field constraints, and a sanitized diagnostic surface.

### Phase 2 — Rust vertical slice

- [x] Keep identity generation deterministic after creation; reject silent
  regeneration and cross-engine profile reuse.
- [x] Bind the stored config to the validated checksummed engine
  artifact checksum and exact engine version.
- [x] Persist the engine artifact SHA-256 in identity schema v3; same-version
  executable substitution now fails before launch.
- [x] Ensure launch uses the approved process controller, keeps profile paths
  confined, and never puts persona contents in argv or logs.
- [x] Prove launch/stop/status and restart through the existing lifecycle
  abstractions without changing Wayfern behavior.

### Phase 3 — Observable capability evidence

- [x] Capture sanitized native readback for main frame, worker, request headers,
  canvas/audio repeatability, WebGL, cookies, extensions, proxy route, restart,
  and multiple-profile isolation on Windows x64.
- [x] Verify checksum-pinned install and keep the
  candidate outside the production runtime catalog.
- [ ] Run the native matrix on macOS arm64; Windows x64 passed locally.
- [ ] Compare results with the Phase 1 parity bar; do not claim feature parity
  from launch smoke alone.
- [ ] Verify that identity and fingerprint config are included only in the
  existing encrypted profile snapshot, then prove Windows-to-Windows restart
  and conflict behavior on two devices. The adapter is not connected to product
  sync/lifecycle yet, so this remains open.
- [ ] Rebase the candidate onto a supported security-current Chromium revision
  and rerun both native platform matrices; Fury v0.4.16 is on Chromium 155 while
  Chrome 156 is stable.

### Phase 4 — Closeout

- [x] Run focused Rust tests, formatting, and the existing adapter contract tests.
- [x] Review the adapter diff for Wayfern changes, unpinned downloads, identity
  leakage, profile collisions, and stale documentation.
- [x] Update this checklist with evidence and residual risks. Production
  promotion remains a separate, explicit release decision.

## Current status

Windows x64 implementation and native smoke are complete for the pinned Fury
core 0.4.16 artifact. The adapter remains an isolated beta candidate: it is not
selectable in the product, has no lifecycle UI/API wiring, and has no macOS
native result in this checkout. It does not meet the bar for replacing Wayfern.
The pinned core reports Chromium 155.0.8059.39; Chrome 156.0.8078.47/.48 became
stable on Oct 20, 2026. The candidate is therefore a full major version behind
and must be refreshed/revalidated before any production consideration.

### Verification evidence (2026-10-27)

- `cargo test -p buglogin-browser --test fury_chromium --test independent_chromium --test runtime_contract --test lifecycle -- --test-threads=1`: 23 passed.
- `cargo build -p buglogin-browser --example independent_chromium_probe`: passed.
- Fury core `0.4.16` Windows x64 release archive SHA-256 verified against the
  upstream release checksum (`3918ae93063bcbc20ab60b176230e11f8f7d762be3460a8f50f935f92beb91ad`);
  extracted browser SHA-256 is checked again by the native smoke.
- Native Windows smoke on the schema-v3 build: config readback,
  UA/client hints/timezone/GPU, worker
  consistency, canvas stress, extension, cookie restart, profile isolation,
  proxy route and process stop all passed (10/10 checks). Graceful stop took
  255/359 ms; forced stop took 15.7 s under the existing process policy.
- The first native run exposed signed 32-bit seed parsing in the pinned core.
  Sending u64 seeds silently disabled canvas/audio variation. Seeds now derive
  deterministically from one profile seed and are encoded as the core's signed
  32-bit values; the repeatability and distinct-profile native checks pass.
- Follow-up contract review found that profile identity was bound to engine
  version but not artifact checksum. Schema version 3 now persists the approved
  executable SHA-256, rejects a different checksum before launch, and verifies
  the validated executable again immediately before spawn. Regression tests
  cover same-version substitution and replacement after adapter validation.
- Initial linker failure was environmental: `D:\BUGLOG~3` (TEMP) had zero free
  bytes. Setting per-process `TEMP`/`TMP` to `G:\BugLogin-Tmp` resolved it.
- `cargo fmt --all -- --check` is not clean because unrelated pre-existing WIP
  elsewhere in the repository has formatting differences. `rustfmt --check` on
  the two changed Rust files passes.
- Browser-family research is not a promise of anti-detection: W3C guidance
  identifies cross-origin correlation and different threat models; the design
  gate is coherent native behavior and measured compatibility, not a vendor
  “undetectable” claim. Chrome UA changes must keep Client Hints consistent.
- The research-time Chrome stable is 156.0.8078.47/.48 while Fury v0.4.16 ships
  155.0.8059.39. This beta is for adapter engineering and local evaluation only;
  its engine is not eligible for production/default use until rebased to a
  supported security-current revision and the native matrix is rerun.

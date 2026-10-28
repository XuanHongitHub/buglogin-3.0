# Independent Chromium adapter

## Scope

Implement separate Rust runtime adapters with persistent per-profile identity,
without replacing Wayfern or changing existing runtime selection. The first
candidate, adryfish/fingerprint-chromium, failed native canvas readback on its
newer engine and is retained only as a tested Windows compatibility baseline.
The current secondary candidate is Fury Chromium; it consumes Fury's published
persona protocol without taking its launcher/server. Both controlled adapters
use the existing `bugium` canonical runtime ID. Retired profiles do not become
launchable merely because these modules exist.

## Authority and isolation

- Rust validates the executable checksum, confines the profile path, persists
  the identity atomically, and builds the launch arguments.
- The identity records its schema, engine family, exact version and executable
  checksum, native operating system, and engine-specific persistent seed.
- Existing identities are immutable during ordinary launch. Config/version
  changes require a newly created profile; never silently rotate or convert an
  existing profile between browser engines.
- The identity/config files contain sensitive profile persona data and store no
  provider credential. Cross-device encrypted-sync inclusion is not yet proven;
  it must be verified before enabling the adapter in product lifecycle.
- Reject arguments that override the identity or select a different profile.
- Preserve Wayfern source, default selection, catalog, and installed profiles.
- No Wayfern/CDP custom identity commands are used by this adapter.

## Evidence and acceptance

Official engine: https://github.com/adryfish/fingerprint-chromium (BSD-3-Clause).
Windows candidate: 150.0.7871.186, official release archive digest
`4d549c326e51ebbabf562fd365eb5380d9d4a81200da2c60f075c688d9a77e03`.
Latest candidate has no macOS asset; macOS parity cannot be claimed from it.

## Native findings, 2026-10-27

- Official Windows 150 reproduced renderer crashes at canvas `toDataURL`, for
  both a random seed and seed 123456. Upstream issues #94/#95 describe the same
  regression. Adapter validation now rejects this engine major explicitly.
- Windows 148 archive was verified against official digest
  `9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579`.
  Native smoke passed fingerprint/audio restart stability, worker/window UA and
  CPU consistency, 25 canvas pixel-readback iterations, extension execution,
  cookie retention, profile isolation, proxy routing, and native process stop.
- Graceful CDP close took 0.7-1.5 seconds. Forced stop follows the existing Windows
  job controller's 15-second grace policy and took about 15.8 seconds. No shared
  process-controller policy was changed.
- The old engine is a controlled compatibility baseline, not a security-current
  production recommendation. Windows/macOS native CI is separate from release.
- Adapters refuse cross-runtime profile reuse. Existing Wayfern profiles are
  not reidentified; no profile conversion/importer exists, and new runtime
  profiles are not enabled in the product UI.
- The earlier `fingerprint-chromium` 148 native CI at
  `875df7249f6b7c3d9f7af119cde76c6f9a392838` passed on Windows x64 and macOS
  arm64: https://github.com/XuanHongitHub/buglogin-3.0/actions/runs/40492401968.
  This is not Fury macOS evidence.
  macOS cleanup waits for verified process exit after CDP close; it does not
  reinterpret an unverified process as stopped. Shared process code is unchanged.
- Override validation follows Chromium switch prefixes (`--`, `-`, and Windows
  `/`), including identity/profile arguments, before creating identity files.

Required checks: argument and checksum rejection, identity persistence/restart,
cross-profile isolation, native fingerprint readback, process-tree stop, cookie
retention, unpacked extension loading, proxy routing, and unchanged Wayfern tests.
Seed equality alone is not evidence of cross-machine/GPU fingerprint equality.

This is an adapter packet, not a production runtime-catalog promotion or a UI
feature. Production selection requires an approved signed artifact and separate
review of the current Wayfern-only selection gates. No production readiness claim
without native Windows/macOS evidence.

## Competitive research and parity bar (2026-10-27)

### Additional primary-source review (2026-10-27)

- [W3C Fingerprinting Guidance](https://www.w3.org/TR/fingerprinting-guidance/)
  frames fingerprinting as a privacy threat with distinct threat models and
  cross-origin correlation risk. BugLogin therefore treats profile identity as
  persistent sensitive state, and evaluates the adapter against declared
  compatibility requirements rather than claiming that a browser is
  “undetectable.”
- [Brave's fingerprinting design](https://github.com/brave/brave-browser/wiki/Fingerprinting-Protections)
  distinguishes API blocking/standardization from seeded randomization; its
  [farbling QA matrix](https://dev-pages.brave.software/fingerprinting/farbling.html)
  explicitly checks page, frame, worker, site, and restart scopes. The adapter
  should test those same consistency boundaries (main frame/worker/header and
  restart) and declare its intended profile scope. Brave's documented
  per-session/per-site scheme is a useful comparison, not a requirement to
  copy: BugLogin's managed profiles need identity continuity across restarts
  and authorized devices, so their privacy and compatibility trade-offs differ.
- [Mozilla's current fingerprinting guidance](https://support.mozilla.org/en-US/kb/firefox-protection-against-fingerprinting)
  documents that reducing exposed information can break site behavior. This
  reinforces measuring both identity coherence and real web compatibility;
  more aggressive noise or blocking is not automatically a better runtime.
- [Chromium's UA override guidance](https://chromium.googlesource.com/chromium/src/+/main/docs/user_agent/README.md)
  explicitly says a User-Agent platform change needs the matching UA Client
  Hints platform change. The Rust validator enforces the native platform,
  browser version, platform Client Hint, and signed 32-bit deterministic noise
  seeds expected by the pinned core; native readback remains the final proof.
- [Mozilla's fingerprint protection overview](https://blog.mozilla.org/en/firefox/fingerprinting-protections/)
  documents a different, privacy-first design: reduce or standardize exposed
  information instead of constructing many per-profile identities. This is a
  useful comparison for product risk and browser anonymity sets, not a
  drop-in implementation for BugLogin's Chromium profile compatibility.
- [Camoufox's fingerprint surface documentation](https://camoufox.com/fingerprint/)
  describes native C++ interception across navigator, headers, screen, fonts,
  WebRTC, WebGL and media/audio, while its [architecture notes](https://camoufox.com/)
  explicitly identify Firefox/Juggler behavior. It is a reference for surface
  coverage and consistency, but requires a separate browser-family adapter.
- [Chrome 156 stable release notes](https://chromereleases.googleblog.com/2026/10/stable-channel-update-for-desktop_02018666648.html)
  put Windows/macOS at 156.0.8078.47/.48 on Oct 20, 2026. Google's [Oct 22
  early stable notice](https://chromereleases.googleblog.com/2026/10/early-stable-update-for-desktop_02123641300.html)
  reports 157.0.8103.15/.16 on Windows and says macOS rollout is still pending.
  Fury [v0.4.16](https://github.com/furyteamtop/fury-antidetect-browser/releases/tag/v0.4.16)
  remains the latest published release as of Oct 28 and ships Chromium
  155.0.8059.39. It is therefore one major behind macOS stable and two behind
  Windows early stable; it fails the production currency gate even though its
  native adapter smoke passes on both supported CI platforms. Refreshing it
  requires a newly pinned binary, native readback, and artifact/license review;
  a version-string edit is not sufficient.
- The upstream [Fury licensing map](https://github.com/furyteamtop/fury-antidetect-browser/blob/main/docs/10-legal-licensing.md)
  separates BSD Chromium patches from AGPL launcher/agent/server/tooling and
  Apache persona schemas. This adapter only implements the published persona
  protocol; it does not copy upstream implementation code or bundle its agent,
  launcher, or team server. Distribution still requires a per-artifact
  component/notices review.

These sources refine acceptance, not vendor endorsement. Vendor test suites and
claims are not independent proof of compatibility or detection resistance.

The adapter must be evaluated as a profile runtime, not just a fingerprint flag
wrapper. Reference implementations were checked against their current primary
documentation/source:

| Reference | Relevant design | Constraint for BugLogin |
| --- | --- | --- |
| ShardBrowser (formerly ShardX) | The current repository describes a Chromium 156 engine, local API/MCP/SDK, profile proxy, QUIC/WebRTC relay, team sync and a device library. | Its launcher is MIT, but its downloaded patched engine is closed and its published terms prohibit embedding it in a commercial anti-detect/fingerprint product without written permission. It is a strong benchmark, not a redistributable runtime. |
| CloakBrowser | Chromium C++ patches, persistent seeds, signed binary download, Playwright/CDP control. | Its binary terms prohibit embedding/bundling/customer control without OEM/SaaS rights. The MIT wrapper license does not grant those binary rights. |
| Donut / Wayfern | Native in-engine fingerprinting, per-profile cookies/extensions/proxy, sync/team locks, templates and profile-level operations. | This is the existing product baseline. Preserve its Rust lifecycle, identity stability, encrypted sync, access checks, and native process ownership; a different engine must not pretend its identity is interchangeable. |
| Camoufox | Firefox C++ patches, real-distribution personas with coherence checks, per-context identity and Playwright over Juggler. | Strong engine-level consistency reference, but it is Firefox/Juggler rather than Chromium/CDP; extension compatibility and existing Chromium profile data cannot be assumed. Keep it as a separate future engine adapter, not a drop-in Chromium replacement. |
| Fury Chromium core | Public Chromium fork with documented native patches, profile seed/persona model, Windows and macOS builds, and engine verification tools. | It is the current secondary candidate in this checkout, not yet production-approved. Pin the upstream revision/artifact and review the exact component licenses/notices before distribution. Do not inherit its launcher, server, account, or sync layer. |

The table records design evidence, not independent claims that any product is
undetectable or universally compatible. Vendor detection-test claims are not an
acceptance signal.

## Recommended path to close the production gates

Keep the current Fury adapter as an isolated engineering beta. Close its gates
through BugLogin-owned runtime artifacts and existing Rust authority; do not
embed another vendor's launcher, team server, or closed browser binary.

### 1. Own the Chromium update cadence

- Create a BugLogin Chromium runtime from an exact upstream Chromium revision
  plus a small, individually reviewed set of engine patches. Fury documents its
  patch series as separable from its AGPL launcher/server, with Chromium-derived
  patches under BSD terms; do a component-by-component license and notice review
  before adopting any patch. The binary must be built by BugLogin CI from
  reviewed source, not downloaded from Fury's release CDN.
- Track Chrome Stable on a scheduled release watcher. Chromium's four-week
  milestone cadence means the watcher should open a source/pin update and
  compatibility job for each milestone, while security fixes get an expedited
  path. A new version must not reach the stable catalog until Windows and macOS
  builds, native readback, profile restart, and compatibility checks pass.
  Google documents the four-week Stable cadence in its [release-channel guide](https://www.chromium.org/chrome-release-channels/); Fury's [Chromium fork notes](https://github.com/furyteamtop/fury-antidetect-browser/blob/main/docs/03-chromium-fork.md)
  describe the patch rebase work and engine build pipeline.
- Keep the current engine available as a last-known-good version. Publish new
  immutable runtime packages under a canary ring first, then promote the signed
  catalog pointer after smoke evidence. Do not rewrite an installed runtime or
  change a profile's bound engine during an ordinary app update.
- The full Chromium build is a real infrastructure cost: Fury records one
  measured macOS arm64 build of about 2 h 42 min and a roughly 39 GB checkout
  plus build output. Reserve persistent, disk-sized Windows and Apple Silicon
  builders; use the existing small CI runners for contract/smoke checks rather
  than pretending that a clean full build is cheap.

### 2. Make synced browser data portable without a manual key prompt

- `profile_projection` already snapshots the profile tree recursively, removes
  known ephemeral Chromium files, and encrypts the resulting manifest objects.
  The identity and fingerprint files under `BugLogin/` therefore fit the current
  encrypted resource payload. Add explicit Windows A → B tests proving those
  files are present after restore and never appear as plaintext in API/object
  storage or logs.
- The missing cross-device case is Chromium cookie/login encryption. A local
  restart test only proves that the same OS account can decrypt its own data.
  Add an engine-level OS-crypt provider using a versioned, domain-separated
  subkey derived from the already-authorized profile resource key. Keep the
  resource key in the Rust OS-vault/sync-key path; pass only the derived
  OS-crypt key to the browser through a second inherited native handle. Never
  put it in argv, React, logs, or a plaintext profile file. This follows the
  separate OS-crypt handle model documented by Fury, but must be implemented in
  BugLogin-owned Chromium source and reviewed independently. The design
  precedent and platform-key trade-offs are described in Fury's [profile sync
  notes](https://github.com/furyteamtop/fury-antidetect-browser/blob/main/docs/06-profile-sync.md).
- Do not create a second recovery-code workflow. The existing authenticated
  device enrollment and wrapped resource key should unlock the derived key
  automatically only after server ACL checks. A bearer without the profile's
  resource-key grant remains insufficient to decrypt data.
- Launch the engine only through the existing `ProfileRuntime` path: reconcile
  the remote manifest, obtain/renew the fenced profile lease, launch, stop,
  produce the encrypted snapshot, commit it, then release the lease. A team
  profile that is not already running cannot safely start during an API outage
  because no server lease can be acquired; an already-running profile follows
  the existing lease-renewal safety window and must stop before ownership expires.
- Bind identity to the exact runtime version and artifact checksum as the beta
  does now. If a synced profile requires a different runtime version, install
  that exact signed version before launch. A profile migration to a newer engine
  must first create a recoverable encrypted checkpoint and remain reversible as
  a unit; never downgrade a migrated Chromium data directory in place.

### 3. Reuse BugLogin's signed runtime distribution

- Extend the existing Ed25519-signed runtime catalog and verified installer to
  publish a Bugium/Fury-family runtime. `Runtime::Bugium`, package checksum
  verification, staged install, and rollback primitives already exist; the
  remaining seams are runtime-catalog/config selection and profile create/launch
  routing, which currently expose Wayfern only. This is a focused extension of
  the existing runtime capability, not a second updater.
- Put platform, architecture, Chromium milestone, source revision, package hash,
  signing key ID, and minimum identity schema in the signed release record.
  Sign macOS app/runtime artifacts with Developer ID and notarize them; sign
  Windows artifacts with Authenticode. Keep the catalog signature/hash check as
  the mandatory install gate, and retain the previous immutable runtime for
  rollback. Publish SBOM, third-party notices, and build provenance beside each
  release.
  Use the existing [Tauri signed-updater contract](https://v2.tauri.app/plugin/updater/)
  for app packages, [Apple Developer ID notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
  for macOS runtime bundles, and [Microsoft SignTool](https://learn.microsoft.com/en-us/windows/win32/appxpkg/how-to-sign-a-package-using-signtool)
  or the current Windows release signer for Windows binaries.
- Keep app version and browser engine version independent. BugLogin app updates
  can ship UI/API fixes without silently changing existing profiles' engine;
  engine releases can be installed independently through the signed runtime
  catalog with explicit profile compatibility status.

### Required proof before changing the default

1. Reproducible source pins and component license review; signed Windows x64 and
   macOS arm64 artifacts; stable catalog verification rejects tampering,
   wrong architecture, downgrade, and interrupted install.
2. Two Windows devices with separate OS accounts: first device opens, edits,
   stops and syncs; second obtains the authorized resource key, restores, opens,
   reads existing cookies/login state, edits, syncs back, and the first reopens
   that committed state. Server-side assertions prove ciphertext-only storage.
3. Two-device lease tests cover active owner, denied second launch, clean handoff,
   heartbeat loss, API outage, process crash, and stale fencing tokens. Expected
   UI states remain `running`, `syncing`, `stopped`, or a specific actionable
   failure; no indefinite `unknown`/orange state.
4. Upgrade and rollback exercise real profiles and extension/proxy configuration
   on both OSes. A failed migration restores the encrypted checkpoint and the
   previous engine without losing the profile's cookies or identity.
5. Re-run native surface, worker, cookie, extension, proxy, process lifecycle,
   sync, and team-lock gates on the exact artifacts being promoted.

Research boundary: [ShardBrowser](https://github.com/ProxyShard/ShardBrowser)
currently documents Chromium 156 and useful cross-device/team workflows, but
its engine is closed and its own terms require permission for embedding in a
commercial anti-detect browser. [CloakBrowser](https://github.com/dmulxw/cloakbrowser)
has a similar binary/OEM restriction. [Camoufox](https://camoufox.com/) is a
useful Firefox engine reference, not a Chromium drop-in because its automation
and profile format differ. These are comparison inputs, not substitutes for a
BugLogin-owned signed runtime.

### Required parity evidence before considering default replacement

### Beta parity snapshot (2026-10-28)

| Capability | Beta evidence | Result / boundary |
| --- | --- | --- |
| Persistent engine-bound identity | Schema-v3 identity is immutable and binds profile seed, exact engine version, OS, and executable SHA-256; regression tests cover substitution. | Pass for local identity and restart. Cross-device portability is not established. |
| Native surface coherence | Native readback covers main frame/worker, UA and Client Hints, canvas, audio, and the pinned core's config. | Pass for the tested pinned Fury build on Windows x64 and macOS arm64. It is not evidence of parity with every Wayfern or reference-browser surface. |
| Profile isolation and data | Smoke checks separate profiles and retain cookies across restart; unpacked extension execution and proxy routing are exercised. | Pass for isolated native smoke. Production storage paths and extension/proxy management remain under existing Wayfern lifecycle. |
| Runtime process lifecycle | Existing Rust process controller launches, reports, and stops the child tree; Windows and macOS native evidence includes stop behavior. | Pass as an adapter/example vertical slice. Product UI, runtime catalog, and profile lifecycle do not select Fury. |
| Sync, locks, and recovery | Identity/config are not wired into the encrypted profile snapshot, remote conflict resolution, or device locks. | Not implemented in this beta; mandatory before product lifecycle use or default replacement. |
| Security updates and distribution | Artifact/version are pinned and checksum-verified for tests; Fury v0.4.16 is based on Chromium 155. | Test-only package passes integrity checks. Security currency, signing, distribution notices, update/rollback are not approved. |

This snapshot defines “beta complete” as an isolated adapter and two-platform
native test path. Unintegrated product capabilities remain explicit promotion
gates and must not be described as completed or enabled in BugLogin.

- A versioned, persistent identity is derived from one measured/coherent device
  persona. It remains byte-stable across restarts and encrypted sync; it is never
  silently regenerated or mixed across engine families.
- Validation covers the full supported schema and cross-field constraints:
  UA/client hints/platform/version, CPU/memory, screen/DPR, GPU and supported
  WebGPU capabilities, locale/timezone, fonts/media, and proxy/WebRTC policy.
  Unsupported values fail with a precise diagnostic rather than being ignored.
- Native readback captures main frame, workers, nested frames, request headers,
  canvas/audio stability, WebGL/WebGPU, media, cookies, extensions, proxy route,
  crash/stop/restart, and multiple-profile isolation. Save sanitized versioned
  evidence per OS and engine revision.
- The full BugLogin lifecycle is wired through Rust authority: create/install,
  launch, running/stopped reconciliation, graceful/forced stop, crash recovery,
  profile lock, cookies/extensions/proxy, encrypted sync and upgrade rollback.
  Wayfern and the signed runtime catalog remain unchanged until a separate gate.
- Release currency and supply chain are explicit: supported browser security
  version, pinned source and artifact hashes/signatures, build provenance,
  third-party notices, update/rollback path, and Windows/macOS native evidence.
- Report gaps by capability and OS. A local smoke pass is necessary but does not
  establish parity with Wayfern or the references above.

## Controlled Rust integration

Use `runtime::independent_chromium::{ChromiumIdentity, IndependentChromiumAdapter}`.
For a dedicated existing profile directory, load `ChromiumIdentity::load` first;
generate only if it returns `None`. Construct the adapter, validate the approved
`RuntimeBinary`, then call the existing `RuntimeAdapter::launch_specification`
and `SystemProcessController`. Read errors are errors, never a reason to generate
a replacement identity. The native example demonstrates this complete path.

The checksum-pinned native smoke is reproducible by dispatching
`.github/workflows/independent-chromium-smoke.yml`. Its artifacts contain engine
metadata and measured checks. It does not modify the release catalog or publish
browser packages. Linux and macOS Intel are not covered by this native matrix.
Canvas/audio stability on one machine is verified; cross-machine identity
portability, cross-OS emulation, external bot-detector scores and parity with
every paid Wayfern fingerprint feature are not proven by these tests.

### Artifact and identity binding

Fury profile identity schema v3 stores the lowercase SHA-256 of the approved
browser executable alongside its exact four-part engine version. Identity
creation receives the expected artifact checksum; adapter binary validation
requires the version and approved checksum to match that identity, hashes the
executable, and the Fury process controller hashes it again immediately before
spawn. Changing a binary while keeping its version therefore cannot silently reuse the
profile identity. This binds the executable, not every neighboring runtime
asset in an extracted package; installation must continue to validate the
archive/package checksum and signed catalog before production distribution.

The native identity uses one random profile seed, domain-separated deterministic
canvas/audio/client-rect seeds, and the pinned core's signed 32-bit wire values.
The exact values are checked against the stored seed on every load, so a partial
or altered identity fails closed instead of creating a new device identity.

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
  put Windows/macOS at 156.0.8078.47/.48 on Oct 20, 2026. Fury [v0.4.16](https://github.com/furyteamtop/fury-antidetect-browser/releases/tag/v0.4.16)
  ships 155.0.8059.39, so it is one major version behind at research time and
  fails the production currency gate even though its Windows adapter smoke
  passes. Refreshing it requires a new pinned binary, native readback and
  artifact/license review; a version-string edit is not sufficient.
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
| ShardBrowser (formerly ShardX) | Chromium engine-level identity, device library, local API/MCP/SDK, profile proxy and team sync. | Its launcher is MIT, but its downloaded patched engine is closed and its published terms prohibit using it in a commercial anti-detect/fingerprint product. Do not bundle or derive from that engine without a separate written license. |
| CloakBrowser | Chromium C++ patches, persistent seeds, signed binary download, Playwright/CDP control. | Its binary terms prohibit embedding/bundling/customer control without OEM/SaaS rights. The MIT wrapper license does not grant those binary rights. |
| Donut / Wayfern | Native in-engine fingerprinting, per-profile cookies/extensions/proxy, sync/team locks, templates and profile-level operations. | This is the existing product baseline. Preserve its Rust lifecycle, identity stability, encrypted sync, access checks, and native process ownership; a different engine must not pretend its identity is interchangeable. |
| Camoufox | Firefox C++ patches, real-distribution personas with coherence checks, per-context identity and Playwright over Juggler. | Strong engine-level consistency reference, but it is Firefox/Juggler rather than Chromium/CDP; extension compatibility and existing Chromium profile data cannot be assumed. Keep it as a separate future engine adapter, not a drop-in Chromium replacement. |
| Fury Chromium core | Public Chromium fork with documented native patches, profile seed/persona model, Windows and macOS builds, and engine verification tools. | It is the current secondary candidate in this checkout, not yet production-approved. Pin the upstream revision/artifact and review the exact component licenses/notices before distribution. Do not inherit its launcher, server, account, or sync layer. |

The table records design evidence, not independent claims that any product is
undetectable or universally compatible. Vendor detection-test claims are not an
acceptance signal.

### Required parity evidence before considering default replacement

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

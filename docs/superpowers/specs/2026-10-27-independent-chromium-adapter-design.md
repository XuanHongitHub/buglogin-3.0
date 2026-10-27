# Independent Chromium adapter

## Scope

Implement a separate Rust runtime adapter with a persistent per-profile identity,
without replacing Wayfern or changing existing runtime selection. The initial
engine under evaluation is adryfish/fingerprint-chromium; Cloak is not required.
The adapter uses the existing `bugium` canonical runtime ID in controlled calls.
Retired profiles do not become launchable merely because this module exists.

## Authority and isolation

- Rust validates the executable checksum, confines the profile path, persists
  the identity atomically, and builds the launch arguments.
- The identity records its schema, engine family, exact engine version, native
  operating system, and randomly generated nonzero 32-bit fingerprint seed.
- Existing identities are immutable during ordinary launch. Config/version
  changes require an explicit future identity migration; never silently rotate.
- The identity is ordinary profile data and must travel in the existing encrypted
  snapshot. No provider credential is stored in it.
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
- New adapter refuses pre-existing Wayfern or untracked Chromium profile data
  without an explicit identity migration. Existing Wayfern profiles are not
  reidentified and new runtime profiles are not enabled in the product UI.
- Native CI at `c4c3c9d4d9f087c93af10e04115ed907dcc0f48b` passed on Windows
  x64 and macOS arm64: https://github.com/XuanHongitHub/buglogin-3.0/actions/runs/40491491634.
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

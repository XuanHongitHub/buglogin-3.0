# Independent Chromium adapter execution

Spec: ../specs/2026-10-27-independent-chromium-adapter-design.md

- [x] Inspect runtime contract, Wayfern boundaries, old adapters and vendor terms.
- [x] Identify an engine independent of Wayfern/Cloak credential services.
- [x] Implement the isolated adapter and immutable profile identity.
- [x] Cover identity persistence, confinement, launch/version/argument guards.
- [x] Verify the official Windows artifact and run a disposable native smoke.
- [x] Exercise fingerprint restart/isolation, cookie, extension, proxy and stop on Windows.
- [x] Run existing Wayfern regression coverage.
- [x] Run isolated Windows/macOS native CI against checksum-pinned test packages.
- [x] Record measured evidence and remaining platform/integration gaps.

Final adapter source: `875df7249f6b7c3d9f7af119cde76c6f9a392838`.
Native CI passed both Windows x64 and macOS arm64:
https://github.com/XuanHongitHub/buglogin-3.0/actions/runs/40492401968.
Local checks: 8 adapter tests, 2 Wayfern contract tests, 31 browser library tests,
7 lifecycle tests, and Biome for both native scripts passed.
Production selection is unchanged; see the spec for engine/platform limitations.

Write scope: `crates/browser/src/runtime/independent_chromium/`, its module export,
focused browser tests, native smoke tooling, and these documents. Concurrent
API/desktop/UI edits belong to other work; do not stage or overwrite them.

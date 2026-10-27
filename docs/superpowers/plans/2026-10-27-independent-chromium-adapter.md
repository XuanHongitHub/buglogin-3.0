# Independent Chromium adapter execution

Spec: ../specs/2026-10-27-independent-chromium-adapter-design.md

- [x] Inspect runtime contract, Wayfern boundaries, old adapters and vendor terms.
- [x] Identify an engine independent of Wayfern/Cloak credential services.
- [x] Implement the isolated adapter and immutable profile identity.
- [x] Cover identity persistence, confinement, launch/version/argument guards.
- [x] Verify the official Windows artifact and run a disposable native smoke.
- [x] Exercise fingerprint restart/isolation, cookie, extension, proxy and stop on Windows.
- [x] Run existing Wayfern regression coverage.
- [ ] Run isolated Windows/macOS native CI against checksum-pinned test packages.
- [ ] Record measured evidence and remaining platform/integration gaps.

Write scope: `crates/browser/src/runtime/independent_chromium/`, its module export,
focused browser tests, native smoke tooling, and these documents. Concurrent
API/desktop/UI edits belong to other work; do not stage or overwrite them.

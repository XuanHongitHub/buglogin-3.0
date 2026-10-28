# Wayfern native identity adapter

- [x] Read BugLogin contracts and current Donut upstream behavior.
- [x] Confirm the Wayfern generation/licensing boundary from official docs and
  the observed `generation_state_unavailable` response.
- [x] Add a dedicated adapter that selects Wayfern's native default without a
  remote generation wait or undocumented identity-file switches.
- [x] Keep the free Chromium integration as BugLogin-owned `FuryCoreRuntimeAdapter`
  / `FuryCoreProcessController`, talking to the standalone core without Donut's
  app, agent, server, or authentication service.
- [x] Fall back to host-native identity with a warning for cross-OS and
  OS-version targets.
- [x] Wire warning propagation into local and managed profile launches.
- [x] Make malformed/unsupported optional fingerprint config warn and fall back
  to native identity instead of blocking profile launch.
- [x] Remove post-launch fingerprint application and dead generator code.
- [ ] Cross-OS identity emulation and reproducible identities require an
  authorized Wayfern entitlement or a separately built and validated core.
  Fury's recent release notes report its cross-OS profile was measured as
  heavily tampered and reverted new profiles to the host OS; do not re-enable
  cross-OS claims based on the earlier prototype.
- [x] Verify Windows + Wayfern 155 native open/stop and targeted regressions.
- [ ] Wayfern 154 and macOS runtime evidence are unavailable in this workspace.
- [x] No release in this implementation packet.
- [x] Refresh the Donut upstream reference to `v0.35.5` at
  `1b045a7c67a1f00a7935e0b31a3394aa2df596fe`, retaining the previous snapshot.

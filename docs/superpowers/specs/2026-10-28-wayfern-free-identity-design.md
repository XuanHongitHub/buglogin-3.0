# Wayfern native identity adapter without entitlement credentials

## Contract

- A profile targeting the current OS opens with Wayfern's browser-native
  identity, without waiting on an identity-generation service.
- BugLogin does not invent a local identity-file format or pass undocumented
  Wayfern command-line switches.
- Do not claim cross-OS or OS-version emulation without an entitlement; keep the
  profile launchable with the host's native identity and an explicit warning.
- Never fabricate identity IDs, borrow another application's token, or claim
  cross-OS / OS-version targets were applied.
- Return a visible warning for unsupported cross-OS / OS-version targets while
  still opening the profile with Wayfern's host-native identity.
- Malformed or unsupported optional fingerprint configuration becomes a
  visible warning during launch; it does not deny profile access.
- Keep filesystem and launch argument ownership in Rust.

## Evidence and boundary

Wayfern's current official documentation states that fingerprint-control CDP
methods require a server-side licence token and generation is restricted to
service accounts. Donut upstream also passes its account token when available.
BugLogin has no authorized Wayfern token source, and this task explicitly has no
license. Therefore local binaries can provide their own native identity, but
they cannot mint a reproducible identity or emulate another OS without that
entitlement. Do not add a local imitation of the server ID.

The upstream reference was refreshed to Donut `v0.35.5` (`1b045a7c67a1f00a7935e0b31a3394aa2df596fe`) at `.tmp/donutbrowser-upstream-v0.35.5`; the previous read-only snapshot remains intact. Its `wayfern_manager.rs` passes `wayfernToken` to generation and rejects a foreign-OS launch with `WAYFERN_CROSS_OS_REQUIRES_PLAN`. This independently confirms the entitlement boundary in the current upstream.

The separate free Chromium-core path remains experimental. Fury `v0.4.16` publishes Windows x64 and macOS Apple Silicon Chromium 155 cores, but its `v0.4.15` release notes say cross-OS profiles measured as heavily tampered (`0.99`) and changed new profiles to the host OS. Keep the BugLogin Fury-core adapter separate from Donut's app/agent/server, preserve the same-OS validation, and do not claim its binary solves reliable cross-OS identity. Its absence of a paid API does not remove the need to satisfy the applicable core, Chromium, branding, and binary redistribution terms.

## Verification

Test host-native default selection, warning and fallback for cross-OS / OS-version
targets, and native-default launch on an installed Wayfern binary. A successful native
default launch does not prove that identity generation or cross-OS emulation is
available.

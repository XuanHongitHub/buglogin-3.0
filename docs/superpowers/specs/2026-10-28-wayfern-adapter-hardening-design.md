# Wayfern adapter hardening

## Status

Implementation in progress. This is a BugLogin-owned adapter flow around the
existing Wayfern runtime, not a browser-engine fork or a second runtime.

## Goals

- Keep Wayfern profile launch independent from other runtime experiments.
- Keep authorized profile launch available when optional fingerprint generation,
  application, or verification fails; return a visible unverified warning.
- Make the local fingerprint control endpoint loopback-only and remove the
  ephemeral-port reservation race by letting Chromium allocate its own port.
- Keep BugLogin's existing runtime contract, profile isolation, sync lifecycle,
  and signed runtime catalog authoritative.

## Boundaries

- Do not copy or redistribute proprietary Wayfern engine or adapter code.
- Use BugLogin-owned code and documented/authorized interfaces only.
- Wayfern's current terms require explicit written permission to run its binary
  outside Donut Browser. Do not claim that the adapter or runtime is releasable
  until the applicable authorization is verified.
- Fingerprint failure is a warning and diagnostic; authentication, profile
  authorization, profile-path safety, sync fencing, and process launch failures
  remain launch gates.
- Do not treat an unverified identity as verified. Persist only identities that
  Wayfern returns and BugLogin successfully validates.

## Behavior

1. Before launch, remove a stale `DevToolsActivePort` marker and configure CDP
   with `--remote-debugging-port=0` and `--remote-debugging-address=127.0.0.1`.
2. After launch, read and validate the browser-generated marker to discover its
   allocated port, then use the existing bounded CDP discovery and fingerprint
   apply/readback flow.
3. If optional CDP/fingerprint setup fails, keep the profile running and return
   `wayfern_fingerprint_unverified` through the existing warning surface.
4. Never fall back to an unloopbacked debugger endpoint or report an unverified
   fingerprint as successful.

## Acceptance

- Tests cover the port-zero launch args, strict marker parsing, loopback CDP URL
  validation, and optional fingerprint failure without profile launch failure.
- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --check` passes.
- The focused desktop runtime tests pass.
- Windows/macOS release permission and runtime evidence remain separate gates.

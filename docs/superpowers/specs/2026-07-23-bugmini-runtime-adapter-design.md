# Bugmini Runtime Adapter Design

## Status

Controlled adapter implementation. Migration activation remains blocked until every approval gate below passes.

## Evidence

- Production inspection on July 23, 2026 found `15` active Bugmini profiles.
- Windows artifact: Chromium `146.0.7680.177`, SHA-256 `0a3e382933e0dcd92b6791c642412a9a570553550165a40c6a8892b43181f97f`.
- Legacy manager uses isolated `--user-data-dir`, proxy routing, Chromium cookie DB, extensions, and CDP.
- Legacy fingerprint behavior is runtime-specific and cannot be represented by the generic launch adapter yet.

## Capability matrix

| Capability | Core adapter | Approval |
|---|---:|---|
| Binary checksum/version | implemented | Windows evidence present |
| Profile isolation | implemented | Windows native create/launch/stop passed |
| Proxy | declared | egress test pending |
| Cookies | implemented | Windows native migrated-session HTTP header passed |
| Extensions | declared | load test pending |
| CDP | declared | Windows disposable-profile smoke passed |
| Fingerprint | not declared | config bridge + parity test required |
| macOS arm64/Intel | not proven | native artifacts/evidence required |

## Rules

- Never map Bugmini records to Wayfern silently.
- Preserve original runtime and `browser_adapter_config` through migration.
- Do not mark Bugmini direct/active until fingerprint, launch/stop, proxy, cookie, extension, Windows, and macOS gates pass.
- Runtime checksum and path validation reuse the verified Chromium-safe Wayfern boundary; launch output keeps canonical runtime `bugmini`.

## Desktop runtime selection

- Desktop bootstrap may configure Wayfern, Bugmini, or both; at least one checksum-pinned runtime is required.
- Profile runtime selects the exact adapter and binary. Missing runtime configuration fails closed; no fallback to Wayfern.
- Local and production launch use the same selection rule.
- Bugmini launch keeps the migrated `runtimeConfig` and parsed `fingerprint`; runtime-specific post-launch parity remains a separate approval gate.
- Windows native smoke must use a disposable profile root and prove checksum, process launch, CDP, process-tree stop, and zero production-profile access.
- Bugmini launch must pass `--user-data-dir=<path>` as one argument. On Windows, the process argument must use a normal Win32 path, not a `\\?\` verbatim path; Bugmini 146 otherwise starts but silently omits persisted cookies. Internal confinement and lifecycle paths remain canonical.

## Windows native cookie evidence — 2026-07-23

- Canonical desktop flow passed: profile create, Chromium cookie DB materialization, Bugmini launch, HTTP request with `Cookie: buglogin_migrated=session-ok`, profile stop, adapter shutdown.
- Root causes were reproduced independently: split `--user-data-dir` arguments exit with code `13` (`Multiple targets are not supported in headless mode`); `\\?\` user-data paths launch but omit persisted cookies.
- Regression coverage: Bugmini argument shape/path normalization, Windows launcher argv boundaries, native lifecycle cleanup with no disposable-profile or Chrome-process leak.

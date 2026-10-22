# Desktop and Hosted API Unix Clock Authority

## Decision

The desktop client, hosted API, and sync coordination code use checked Rust clock boundaries for Unix seconds, milliseconds, and nanoseconds. A pre-epoch system clock or an unrepresentable millisecond timestamp returns the canonical retryable HTTP 503 `clock_unavailable` Problem. Authentication expiry, MCP auth/audit, HTTP auth/control routes, sync conflict/Redis event timestamps and tombstone retention/import, managed-lease expiry validation, cookie mutation timestamps, profile state and proxy-assignment timestamps, generated profile IDs, proxy creation timestamps, active-device authorization, package journals, and recovery workers fail closed instead of substituting zero.

## Compatibility

This changes only failure behavior when the local wall clock cannot represent a valid Unix timestamp. Normal timestamp units and API payloads remain unchanged.

## Acceptance

- Seconds and milliseconds share the same error code and correlation behavior.
- Authentication expiry, sync snapshot retention, and pre-acquired lease validation use the shared helper.
- Cookie lifecycle and profile/proxy metadata timestamps use the same helper; proxy creation checks time before writing credentials to avoid orphaning a vault secret on clock failure.
- MCP authentication/audit timestamps and hosted active-device/retention checks use the API clock helper with explicit error propagation.
- HTTP request handlers propagate clock Problems through their correlation ID; internal API state and worker paths propagate typed `Problem` errors.
- Sync conflict detection and Redis manifest event creation reject pre-epoch or unrepresentable timestamps instead of converting them to epoch zero.
- Cookie-job timestamp accessors return a checked error rather than epoch zero; profile proxy-assignment timestamps use the API clock boundary and return retryable `clock_unavailable` on failure.
- Cookie stage restore/import/expiry fails closed on invalid time and checks TTL overflow; process ownership tokens use OS randomness rather than a timestamp fallback.
- API persistence timestamps in milliseconds use checked millisecond conversion directly; they do not saturate a seconds-to-milliseconds conversion.
- Auth/session, device authorization, lockout, recovery, quarantine, and reservation deadlines reject arithmetic overflow before any durable mutation.
- Desktop device authorization, refresh thresholds, local cookie leases, and profile tombstone retention also reject unrepresentable deadline arithmetic.
- No production call site in these paths silently defaults a failed epoch conversion to zero.
- Unit and desktop contract checks pass in the consolidated verification pass.

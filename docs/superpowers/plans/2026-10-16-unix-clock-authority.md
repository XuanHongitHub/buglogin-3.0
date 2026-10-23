# Unix Clock Authority Plan

- [x] Add one desktop clock boundary for Unix seconds and milliseconds.
- [x] Apply fail-closed timestamps to auth expiry, sync snapshot tombstones, managed leases, cookie lifecycle, and profile/proxy metadata.
- [x] Apply checked time to MCP auth/audit and hosted active-device, membership activation, and quarantine-retention decisions.
- [x] Add regression source for pre-epoch handling and normal unit conversion.
- [x] Replace production `http::now()` call sites with correlation-aware request time or typed internal clock Problems; keep the test-only clock helper isolated.
- [x] Make package/sync journal timestamps and the hosted recovery worker fail closed on invalid or unrepresentable system time.
- [x] Make sync conflict and Redis manifest event timestamps fail closed on invalid or unrepresentable system time.
- [x] Remove the cookie-job epoch-zero accessor fallback and route proxy-assignment timestamps through the checked API clock.
- [x] Fail closed on cookie-stage expiry clock errors and use OS randomness for Unix process identity tokens.
- [x] Replace saturating Unix-seconds-to-milliseconds conversions in API persistence paths with checked millisecond timestamps.
- [x] Check auth/session, device authorization, lockout, recovery, quarantine, API-token, and reservation deadline arithmetic before persistence.
- [x] Check desktop authorization, refresh, local cookie lease, and sync tombstone deadline arithmetic.
- [x] Use the shared checked RFC3339 clock helper for profile creation, group/share/proxy-assignment timestamps, and migration timestamps; add regression source for canonical UTC formatting.
- [ ] Run the consolidated verification pass after implementation is complete (deferred by user).

Implementation is source-complete; tests and build have not been run.

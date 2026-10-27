# Authoritative Profile Lock and Runtime Status

## Goal

Show a profile lock only when the coordination service confirms a live lease. Keep process activity and cloud synchronization as separate server-reported states so the UI never guesses that a stopped profile is syncing merely because its lease remains held.

## Contract

- The atomic lease acquire remains the sole authority for Start. A held lease rejects competing launches; a status read is informational and must not be used as a preflight lock check.
- Runtime projection states are `running`, `syncing`, and `stopped`. The projection is informational; lease ownership and fencing still authorize writes.
- The device reports `syncing` after confirming its browser process tree stopped and before it starts cloud synchronization. It reports `stopped` after successful synchronization and before releasing its lease.
- A held lease with an explicit `syncing` projection is shown as syncing. A held lease with `running` projection is shown as running. Missing or mismatched projection does not erase the confirmed lease; it is shown as lock held with status unavailable/stale.
- Failed status reads preserve the last confirmed lease for display and mark it stale. They never turn a known held lock into an unlocked state. A confirmed empty lease response may clear it.
- The existing per-profile REST/desktop reads remain compatible. The desktop status UI uses one bounded workspace batch snapshot (up to 250 requested profile IDs) to fetch lease and runtime phase together. The API rechecks access per profile and caps concurrent coordination reads at 32.

## Failure behavior

- If the browser stops but sync fails, keep the lease/fencing recovery behavior already owned by the runtime coordinator and report the last confirmed phase as syncing until the lease is released or status is reconciled. Never claim synced.
- A timeout or unreachable owner is not proof that the process stopped. Keep the confirmed lease until server lease expiry/reconciliation and identify stale status distinctly.
- No frontend state can grant or revoke write authority.

## Scope

Add an authenticated, lease-authorized `runtime/syncing` projection route and desktop client method; publish it in the managed stop/sync path; add a bounded, permission-filtered workspace status batch; update profile lock resolution and polling failure handling. Preserve existing runtime projection fields and old started/stopped endpoints.

## Acceptance

- Existing clients continue to read `running`/`stopped` projections.
- A live lease plus explicit syncing projection is displayed as syncing.
- A failed read does not remove a previously confirmed lock; a successful empty lease read does.
- Competing Start remains governed by atomic server lease acquisition.
- Targeted typecheck/build and API/desktop compilation pass.

# Production Data-Root Inspection — 2026-07-23

## Scope

Read-only inspection of `%LOCALAPPDATA%\BugLogin`. No source file, profile, vault, browser process, or control-plane record was modified.

## Evidence

- First and second raw snapshots matched: `9da4587dead42f0dc4e0a22c25196338292b15ad06ae7fddfcc4a12990013912`.
- Snapshot size: `1,961,345,035` bytes, `15,332` files.
- Inspect result: `15` profile metadata records, `88` proxy records, `0` invalid records, `0` dangling references, `4` optional-reference warnings.
- Secret scan found `95` secret-bearing fields by presence only; values were not emitted.
- Runtime classification: `bugmini`, `15` records, `runtime_adapter_not_approved`.
- Cutover eligibility: `blocked`.

## Interpretation

The source is stable across two snapshots. Migration must not proceed: all inspected profiles use `bugmini`, while BugLogin 2.0 currently approves Wayfern launch only. A control-plane export is also required before workspace/member/resource/grant migration. No package, destination write, or cutover was attempted.

## Next gates

1. Approve and implement the `bugmini` adapter or an explicit deterministic regeneration policy.
2. Export the legacy control plane through its supported admin path.
3. Re-run inspect with the control export, then perform two isolated production-clone rehearsals.
4. Preserve this snapshot as the source freeze reference; repeat before cutover.

## Status correction — 2026-08-21

BugLogin 2.0 now contains a checksum-verified `bugmini` runtime adapter and API lifecycle dispatch. The findings above remain the historical result of the 2026-07-23 inspection; current inspection keeps `bugmini` partial until a verified runtime artifact is provisioned. Cutover remains blocked until native Windows/macOS launch evidence passes, the legacy control-plane export is supplied, and two isolated rehearsals prove verification plus rollback.

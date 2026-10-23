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

## Read-only source reinspection — 2026-10-13

- The owner workstation's existing protected clone at `F:\BugLogin3-Migration\raw-source-snapshot-2026-10-11` has executable version `1.1.84`. Two stored snapshot manifests match each other and a fresh pair of read-only snapshots of `%LOCALAPPDATA%\BugLogin`: `e1528c68371e6bf0eb56c94332d0171997d2495f05322022600f78eca057a222`, 14,615 files, 1,903,739,071 bytes. The source process was not running during this verification.
- The current migration CLI inspect result for that clone found 14 valid profile metadata records, 88 proxies, one invalid profile (`profile_metadata_missing`), four optional-reference warnings, no dangling references, and 95 secret-bearing fields by presence only. All 14 valid profiles classify as `bugmini:partial`; cutover eligibility is `blocked`.
- The CLI needed an explicit inspection identity because the legacy root has no root `version.json`. `1.1.84` was verified from the executable; `legacy-local-acer-20261011` was supplied only as a provisional, inspection-only source ID. It has not been used to export/import, and a stable source ID must be reconciled with the production/control-plane export before deterministic package IDs are generated.
- This is a local profile-data clone, not a hosted production clone. It does not include the separately required organizations/workspaces/memberships/grants export. No source files, browser profiles, vault entries, databases, or 1.0 services were modified; no migration package, destination write, or cutover was attempted. The inspect report remains under the owner-protected `F:\BugLogin3-Migration` folder.

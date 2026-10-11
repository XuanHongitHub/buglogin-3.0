# Migration Rehearsal Reconciliation Gate

## Scope

Require each synthetic migration rehearsal run to execute the full package-to-rollback chain and the existing destination reconciliation before rollback. A rehearsal may not report success when activation counts pass but reconciliation is unverified.

## Contract

- Call `reconcile_activation` after repeat activation and before rollback.
- Require `ReconciliationStatus::Passed`.
- Record expected and activated profile counts in redacted evidence.
- Cover package build, signing, verification, dry-run planning, staging, confirmed cutover, activation, reconciliation, activation rollback, and cutover rollback in one synthetic test.
- Keep source immutable and preserve existing foreign destination data checks.

## Non-goals

- No desktop/UI changes.
- No production migration authorization.
- No new reconciliation implementation.

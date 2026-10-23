# Group Scoped Sharing Design

## Goal

Preserve workspace group shares as typed, auditable grants in BugLogin 3.0, with one Rust authorization authority and migration behavior that never silently changes the target scope.

## Contract

- A group is a workspace-owned collection of profiles, not a profile resource kind.
- Group grants target a `GroupId` and active workspace `MemberId` explicitly.
- Membership is dynamic: profile assignment determines current group access; reassignment takes effect on the next protected request.
- Profile clone copies descriptive/runtime metadata only. It must clear `groupIds`; group assignment is a separately authorized ACL mutation and clone permission alone must not grant group recipients access to a new profile.
- Grant evaluation runs only after active workspace membership is established. Token scope and explicit capability deny win over any group allow.
- Legacy `full` maps to the actions represented by the existing profile-sharing contract (read, update, execute, share); `run_sync_limited` maps to read and execute. No group grant includes delete/manage.
- Revoked source grants remain inactive provenance. Ambiguous member/group/workspace mappings fail closed with row-level diagnostics.
- Current 1.0 clone evidence shows group grant IDs match workspace groups and profiles carry group assignments. The legacy runtime implementation is absent from available source; migration reports this limitation and requires owner review of the dynamic 3.0 mapping.
- All protected profile operations call one shared hosted evaluator. Direct profile shares retain their existing behavior.
- Share and revoke routes authorize `CAP-SHARE-001:manage` before returning profile-binding, recipient-membership, or key-rotation outcomes. Group-share creation authorizes the group mutation before checking whether the target member is active, so these APIs do not expose object or membership existence to unauthorized callers.
- Encrypted-profile rotation fences are set only after share/revoke authorization, target, and revision validation succeeds. A failed mutation must not leave rotation-pending side effects.
- REST, MCP, Tauri, and web expose adapters only; the Rust service owns validation, authorization, revision, idempotency, audit, and persistence.

## Persistence and migration

Group grants are persisted in the same versioned control aggregate as workspace groups and resource grants. Import validates the group and recipient workspace before committing. Import/replay must be atomic and rollback must restore the exact pre-import control aggregate. Old profile-grant records deserialize unchanged.

## Verification

Require domain permission tests, service tests, HTTP/MCP/Tauri parity, restart persistence, and migration dry-run/import/replay/rollback coverage for dynamic reassignment, revoked grants, full vs run-sync-limited actions, denies, token scopes, and workspace isolation. Production activation remains blocked until the source snapshot and migration mapping report are reviewed.

## Scope

This change covers group grants and group-share migration only. UI work follows `docs/design/ui-authority.md` and the approved Donut reference; it cannot ship as a fake or placeholder control.

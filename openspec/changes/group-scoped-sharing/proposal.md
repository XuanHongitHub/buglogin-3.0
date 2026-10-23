# Group Scoped Sharing and Migration Parity

## Why

BugLogin 3.0 stores profiles in workspace groups, while its share grants currently target individual profile resources. The 1.0 production clone contains group-target grants that the migration mapper correctly rejects. Treating those grants as profile grants would lose their group scope and can silently overgrant or undergrant access.

## Change

Add typed, workspace-scoped group share grants to the Rust authority. A grant applies dynamically to profiles currently assigned to its group, and is re-evaluated whenever a profile is reassigned. Profile access still requires an active workspace membership; token scopes and explicit capability denies take precedence. Preserve revoked grants as inactive migration provenance. Carry groups and group-target grants through canonical migration export, dry-run, import, persistence, and rollback.

REST, MCP, Tauri, and web adapters must call the same service and return the same authorization and revision outcomes. No adapter may implement group membership authorization independently.

## Compatibility and migration

Keep existing profile-target grants readable and behaviorally unchanged. Add an explicit target discriminator for group grants; do not encode a group as a profile resource or reuse an unrelated ID type. Migration must validate that every group target belongs to the stated workspace and that every recipient resolves to the intended workspace member. Unknown/mismatched targets remain rejected with a row-level report; no grant is dropped or broadened silently.

The source clone proves each legacy group grant points to a workspace group and profiles carry group assignments, but the legacy runtime source was not available to verify all temporal permission semantics. The 3.0 contract therefore defines future group grants as dynamic. Migration evidence must state that mapping and preserve source IDs, access mode, revoked state, and source snapshot hash for owner review.

## Risks

- Dynamic group membership means later profile assignment changes the set of profiles visible through a group grant. The behavior must be visible in the UI and covered by authorization tests.
- Full legacy grants include read, update, execute, and share actions. Every protected profile action that accepts a direct share must use the shared group-aware evaluator without bypassing denies, token restrictions, or workspace checks.
- OpenSpec implementation spans WP-33 resource grants and WP-39 migration. Keep the stages sequential and update both contracts before activating imported grants.

## Out of scope

- Changing individual profile share semantics.
- Public signup, billing, email invitations, or recovering legacy passwords.
- Activating a grant whose target/member/workspace mapping is ambiguous.

# Group Sharing Spec Delta

## ADDED Requirements

### Requirement: Group grants are workspace-scoped and dynamic

The system MUST represent a group share with a typed group identifier, workspace identifier, recipient member, allowed actions, effect, revision, and active/revoked state. A group grant MUST apply only to profiles currently assigned to that group in that workspace. Reassignment MUST immediately change the set of profiles covered by the grant.

#### Scenario: Current group member can use a shared profile

- GIVEN an active member has an allow grant for a group in workspace W
- AND an active profile in workspace W is assigned to that group
- WHEN the member requests an action allowed by the grant
- THEN the request is authorized subject to capability, token-scope, and deny checks

#### Scenario: Reassignment removes group-derived access

- GIVEN a member can access a profile only through group G
- WHEN the profile is moved out of G
- THEN subsequent requests through G are denied

#### Scenario: Cross-workspace group access is denied

- GIVEN the grant, group, member, and profile do not all resolve to the same workspace
- WHEN the member requests the profile
- THEN the system denies access without revealing cross-workspace target existence

### Requirement: Explicit deny and token scope take precedence

The system MUST evaluate token scopes and matching explicit capability denies before group-share allows. A group share MUST NOT grant access to an inactive or non-member account, bypass a workspace boundary, or grant actions absent from its action set.

#### Scenario: Deny overrides group allow

- GIVEN a member has a group allow and a matching explicit deny
- WHEN the member requests that action on a profile in the group
- THEN the request is denied

#### Scenario: Token scope limits a group grant

- GIVEN a group grant allows an action but the session token lacks that action scope
- WHEN the member requests that action
- THEN the request is denied

### Requirement: Migration preserves group grant evidence

The migration pipeline MUST carry group entities, profile-to-group assignments, active and revoked group grants, access mode, source grant ID, and source snapshot provenance. It MUST reject ambiguous or cross-workspace mappings with explicit diagnostics and MUST NOT silently drop, broaden, or convert a group grant into a set of profile grants.

#### Scenario: Revoked grant remains inactive provenance

- GIVEN a legacy group grant is revoked in the immutable source snapshot
- WHEN the migration package is imported
- THEN the grant is retained as revoked provenance and grants no access

#### Scenario: Unknown group target is rejected

- GIVEN a legacy group grant references a missing group or a group in another workspace
- WHEN migration validation runs
- THEN that row is rejected and the report identifies the source grant without activating a substitute grant

### Requirement: All adapters share one authority

REST, MCP, Tauri, and web adapters MUST invoke the same Rust group-grant service. Authorization, revision checks, idempotency, audit, and error behavior MUST be consistent across adapters.

#### Scenario: Adapter parity

- GIVEN equivalent authenticated requests through REST and MCP
- WHEN each changes the same group grant with the same precondition
- THEN both use the same service rules and produce equivalent outcomes

## MODIFIED Requirements

None.

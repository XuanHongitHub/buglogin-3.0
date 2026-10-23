# Encrypted Group Profile Sharing Design

## Objective

Make encrypted profiles usable through BugLogin's existing dynamic group grants while preserving the server's inability to see profile keys. Use one Rust authority for access decisions, key recipients, and sync fences.

## Contract

- Effective encrypted-key recipients are the union of active direct profile `read`/`execute` grants and active group `read`/`execute` grants for the profile's current group assignments.
- Recipients must be active members of the same workspace. Direct resource denies, group denies, and explicit `profile.read`/`profile.launch` capability denies take precedence in recipient calculation. Each requesting principal's token-scope restrictions are enforced by action authorization. Owner rotation recipient resolution must omit capability-denied members so it cannot deliver an envelope to an account that cannot retrieve it.
- Each effective recipient device needs a registered active public key and one profile-key envelope bound to workspace, resource, device, key version, and confirmed key fingerprint.
- Grant, revoke, group-assignment, and member activation/suspension mutations re-evaluate every affected encrypted profile. They persist the ACL/member mutation and rotation-pending fence atomically. Revocations stop new authorization immediately.
- Direct resource-level `read`/`execute` grant creation and revocation on a bound encrypted profile also persist the ACL mutation and rotation-pending fence atomically across REST and MCP; deny grants follow the same rule because they change the effective recipient set.
- Device-key enrollment/revocation for an effective recipient triggers envelope distribution or a durable rotation-pending fence, so a new device never relies on a different device's envelope.
- While pending, encrypted payload sync is denied. The owner re-encrypts the complete resource generation and prepares envelopes from keys kept in the OS vault.
- At manifest commit, the server recomputes the exact effective device set under the control mutation lock. It rejects stale or incomplete envelope sets, stores the new manifest and envelopes in one aggregate, deletes envelopes for former recipients, and clears the pending fence in that same commit.
- Active recipients without registered active devices or complete key coverage fail closed. Bounded operations reject the entire ACL operation; no partial grant/profile batch is committed.
- Before an encrypted group grant, profile-group replacement, member activation, or new device-key enrollment is committed, the resulting effective recipient set must fit the rotation limits (256 members, at most 64 active devices per member). Over-limit changes are rejected and the prior control aggregate is restored, so an ACL/device change cannot create a rotation fence the owner API cannot process.
- Migration control imports preserve encrypted managed-profile group grants even when the legacy package has no device-key envelopes. The same atomic control-state commit marks each affected resource rotation-pending; sync stays denied until the owner explicitly confirms recipients and completes a full key rotation. Imports must never synthesize envelopes or silently clear this fence.

## Data flow

1. Rust mutation service changes group grant or profile assignment and marks affected encrypted resources pending.
2. Owner desktop sees the pending profile and fetches the effective-recipient/device-key snapshot.
3. Desktop displays every recipient device fingerprint and requires explicit owner confirmation of the complete snapshot. Rust binds that confirmation to the exact recipient/device set and rejects it if the set changes before execution.
4. For a running profile, desktop stops the browser and syncs its stopped projection. For an idle profile, desktop hydrates the encrypted generation into the locked projection cache without starting a browser, acquires an update lease, and keeps that lease renewed through upload and commit.
5. Desktop rotates resource keys, encrypts all current payload objects, and creates one HPKE envelope per effective device.
6. Rust re-resolves recipients and key fingerprints, validates exact coverage and the successor manifest, and commits manifest, envelopes, audit/idempotency receipt, and fence clear atomically.
7. Each authorized recipient device retrieves only its own current envelope. Sync remains unavailable to former recipients and to anyone while rotation is pending.

## Failure behavior

- Group or membership mutation failure rolls back both ACL and rotation fences.
- Revoking a device-bound account session fences affected encrypted resources before the session is revoked. If fence persistence fails, the session remains active; if session revocation fails after fencing, the conservative fence remains until the owner completes rotation.
- Concurrent ACL/device/key changes make the owner commit fail with a retryable stale-recipient conflict; no envelope or manifest is partially published.
- Missing owner device or missing recipient public key leaves the profile fenced and returns a typed, actionable error.
- Old copied plaintext or keys cannot be erased remotely; a successful new generation prevents former recipients from decrypting later payload updates.
- If hydration, lease acquisition, projection, or upload fails for an idle profile, no browser is launched and the pending rotation fence remains. A still-live sync lease remains renewable for retry and is released only after commit or explicit cleanup; an expired lease must be reacquired with a new fencing token.
- Migration rejection preserves source package and destination state for operator remediation.

## Ownership and gates

IO-CONTROL leads WP-33; IO-AUTO integrates WP-19/WP-47; IO-DATA integrates WP-36/WP-37 and migration constraints; IO-UI implements desktop orchestration under WP-21/WP-41 after the contract stabilizes; IO-QA owns WP-48/WP-50. No overlapping packet writers. UI changes require `docs/design/ui-authority.md`, the pinned Donut reference, and visual/interaction evidence.

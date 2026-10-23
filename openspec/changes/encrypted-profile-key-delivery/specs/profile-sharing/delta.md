# Profile sharing key delivery delta

## ADDED

### Requirement: Encrypted profile shares publish usable recipient keys atomically

For an encrypted profile, `profile.share` MUST include exactly one HPKE envelope for every active device session of the active recipient member. Each envelope MUST bind the profile's current workspace, resource, and manifest key version. Each envelope MUST include a confirmed fingerprint equal to the registered active device public-key fingerprint. The API MUST atomically persist the share grants and opaque envelopes in one control aggregate mutation. If any validation fails, neither grants nor envelopes may be published.

### Requirement: Recipient key envelopes are device-private API results

The API MUST select the recipient device from the authenticated device-bound bearer, never from a caller-supplied device ID. Envelope retrieval MUST require current profile read authorization and return only the envelope for that authenticated device. A revoked grant, inactive member/device, pending key rotation, or bearer without a device binding MUST fail closed.

### Requirement: Revocation removes future envelope access

Profile share revocation MUST remove that member's stored envelopes in the same durable mutation as grant revocation and the resource rotation-pending fence. It MUST NOT claim to erase keys or plaintext previously copied by a recipient.

## MODIFIED

Encrypted profile sharing through direct and dynamic group grants is available only when the exact effective recipient device set has validated envelopes and fingerprint confirmations. The set is the union of active direct grants and active group grants for the profile's current group assignments, after workspace membership and deny rules are applied.

### Requirement: Group ACL changes fence encrypted profiles

Creating or revoking a group read/execute grant, changing an encrypted profile's group assignments, or activating/suspending an effective recipient MUST update the ACL/member state and mark every affected encrypted resource rotation-pending in the same durable control mutation. Revocation and reassignment MUST remove authorization immediately. While rotation is pending, encrypted payload sync MUST fail closed until the owner publishes a fully re-encrypted generation and the exact replacement envelope set.

### Requirement: One effective recipient set governs encrypted access

Encrypted profile authorization, key-rotation recipient enumeration, envelope validation, and manifest publication MUST use one Rust-owned resolver for direct and group recipients. A group deny, explicit capability deny, token scope restriction, inactive member, wrong workspace, or inactive device MUST NOT receive an envelope. Rotation MUST reject an envelope set that is missing, stale, duplicate, extra, or bound to a different key version, resource, workspace, device, or registered key fingerprint.

### Requirement: Rotation removes envelopes for former recipients

Publishing a new encrypted manifest generation MUST replace the full envelope set atomically with the manifest and rotation-fence clear. The new set MUST contain exactly the current effective recipient devices. Envelopes for members or devices that no longer qualify MUST be removed in that same mutation.

### Requirement: Migration cannot create keyless encrypted group access

Migration control import MUST reject an encrypted managed profile with active group read/execute grants unless the imported package contains validated envelopes for the exact effective recipient device set and key version. A control-only package that lacks encrypted key state MUST NOT activate such a grant.

## NOT MODIFIED

The server never receives plaintext resource keys or private device keys. Revocation still fences encrypted sync until a complete fresh-key generation and envelopes for remaining readers are published.

# Encrypted profile key delivery for direct and group shares

## Why

BugLogin stores device public keys and implements the HPKE primitive. Direct encrypted shares have device-bound key envelopes, but dynamic group shares cannot currently deliver keys. Extend the same server-authorized key-envelope contract to the exact effective recipients of direct and group access. ACL changes must fence encrypted sync until the profile owner publishes a complete new key generation.

## Scope

- Extend the canonical `profile.share` request with per-device HPKE envelopes and explicit fingerprint confirmations for encrypted profiles.
- Allow an account to share to its own active workspace membership for cross-device access; this remains member-wide and must review the complete active device-key set.
- Persist grant metadata and opaque envelopes in the same durable control aggregate mutation.
- Validate exact workspace, resource, active member/device, registered key fingerprint, and manifest key version before publishing.
- Add recipient-device-only envelope retrieval and delete recipient envelopes atomically on revocation.
- Preserve metadata-only sharing for unencrypted profiles and reject partial or extra envelope sets for encrypted profiles.
- Resolve encrypted profile recipients as the union of active direct and dynamic group `read`/`execute` grants, with workspace membership and deny/token rules enforced by the canonical Rust authority.
- On encrypted profile group grant, assignment, or recipient membership changes, atomically mark every affected profile rotation-pending with the ACL/member mutation. Revocation removes access immediately; encrypted sync stays fenced until the owner rekeys.
- Ensure active device-key enrollment/revocation for an effective recipient triggers exact envelope delivery or a durable key-rotation fence; one device must never reuse another device's envelope.
- Reuse one exact recipient/device resolver for authorization, rotation snapshots, envelope validation, and manifest publication. A successful rotation replaces the full envelope set and removes envelopes for recipients that no longer have access.
- Keep migration import fail-closed for encrypted managed profiles when the package lacks matching key envelopes; never create an active encrypted group ACL without usable keys.

## Out of scope

- Changing cryptographic algorithms or allowing the API to see profile keys.
- A new sharing screen or visual redesign. The existing profile-access dialog may expose the current-account recipient using existing Donut components and must pass the established accessibility/screenshot gate.
- Claiming production readiness before desktop confirmation, hosted persistence, revocation/rekey, and staging E2E evidence pass.

## Ownership / integration

- IO-CONTROL leads the integration across WP-33 resources/grants and group ACLs.
- IO-AUTO owns REST/OpenAPI/MCP contract parity in WP-19 and WP-47.
- IO-DATA owns manifest/key-envelope atomicity and pruning in WP-36/WP-37.
- IO-UI owns desktop owner rekey, fingerprint confirmation, and recipient recovery in WP-21/WP-41 after the Rust contract stabilizes.
- IO-QA owns adversarial HTTP, migration, and vertical E2E evidence in WP-48/WP-50.
- No S3 or BugIdea resources are touched.

## Acceptance

- Existing unencrypted share behavior remains compatible.
- Sharing to the current account's own active membership applies to all of that account's active registered devices; encrypted shares require exact fingerprint confirmation for every device.
- Encrypted share rejects missing, stale, duplicate, extra, wrong-scope, wrong-fingerprint, or inactive-device envelopes with typed errors and no ACL mutation.
- A successful share persists grants and envelopes together across API-state reopen.
- A recipient can retrieve only its own envelope after current profile-read authorization; unbound/web sessions, other members, and revoked shares cannot retrieve it.
- Revocation removes that member's envelopes and activates the rotation-pending fence in the same aggregate mutation.
- The exact effective recipient set includes direct and current group readers; group assignment/grant changes immediately fence affected encrypted profiles.
- Rotation commits reject missing, stale, duplicate, extra, inactive, cross-workspace, or wrong-fingerprint device envelopes, and atomically prune envelopes for removed recipients.
- Migration cannot activate encrypted group grants without envelope coverage for the imported key generation.
- API, OpenAPI, generated client, compatibility, and focused persistence/authorization tests pass.

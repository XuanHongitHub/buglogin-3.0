# Atomic Profile Proxy Assignment Design

## Goal

Allow an existing managed profile to change its selected workspace proxy without exposing metadata that disagrees with the active browser-data manifest. The selected proxy and manifest dependency become active in one durable control-state mutation.

## State machine

1. `profile.proxy.assignment.set` validates profile update permission, expected profile revision, target proxy workspace/read permission, and non-running profile state. It records a pending assignment without changing `Profile.proxy_id` or the active manifest.
2. The desktop sync worker reads the pending assignment (where `proxyId: null` explicitly means unassign), prepares the proxy dependency and resource-scoped encrypted credential envelope from the OS vault, snapshots browser data, and uploads immutable objects. The set input requires the `proxyId` field; omission is invalid.
3. `profile.manifest.commit` validates the write lease/fencing token, expected profile revision, pending assignment, proxy revision/read permission, envelope scope, and manifest successor.
4. In the same control aggregate/idempotency/audit transaction, commit activates the new manifest, updates `Profile.proxy_id` and its revision, and consumes the pending assignment.
5. Failed upload/validation leaves the current profile, manifest, and pending assignment unchanged. A replay after an ambiguous successful commit returns the stored receipt; manifest reconciliation remains the desktop recovery path.

## Invariants

- The profile never advertises a proxy that its current active manifest does not reference.
- A pending assignment is not effective for launch and does not change the current `Profile.proxy_id`.
- One pending assignment exists per profile; replacing it requires the current profile revision and an idempotency key.
- The proxy must be active, readable by the actor, and in the bound profile workspace. Credential data stays in the OS vault and travels only as an encrypted profile-scoped envelope.
- Profile update authorization, resource lease/fencing, idempotency, audit, revision CAS, and manifest generation checks remain server-side.
- The API transaction changes profile metadata, manifest, audit, idempotency receipt, and pending-assignment state together.
- Old clients may continue committing manifests only when their dependency matches the active profile. They cannot consume or override a pending assignment.

## Scope and compatibility

Add canonical set/get assignment commands with REST and MCP adapters; use the same typed Rust service. Desktop sync reads the assignment and submits the profile revision with manifest commit. New fields are additive; v1 profile/manifest reads remain valid. Web/desktop controls remain separate UI work subject to the binding UI authority.

## Explicitly out of scope

Proxy inventory editing, key distribution for profile sharing, and production staging/release evidence. Those remain independently gated.

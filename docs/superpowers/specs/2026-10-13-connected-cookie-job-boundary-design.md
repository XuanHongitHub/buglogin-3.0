# Connected Cookie Job Boundary Design

## Problem

The Rust router currently deserializes the storage/service enqueue struct directly. That exposes server-owned actor, idempotency, and timestamp fields as wire inputs even though the handler overwrites only some of them. The router also persists jobs without checking profile ACLs, returns HTTP 200 despite an accepted-job receipt, and the REST paths have no canonical OpenAPI contract.

## Decision

Keep HTTP input separate from the durable job model. The caller may provide command/workspace/device/profile/generation and only the optional source or opaque staged reference required by the command. Derive actor from authenticated session, idempotency from the header, and creation/expiry from the API clock with the existing ten-minute contract window. Reject unknown fields.

Authorize the destination against its profile binding and command capability before persisting. For `cookie.copy`, independently authorize source profile read access and require its bound workspace to match the supplied source workspace. Cookie contents, filesystem paths, vault data, and device-side execution proof never enter this API.

Document the actual queue endpoints as a control-plane contract. Enqueue returns HTTP 202; device inbox/claim/ack/complete require device bearer auth. Inbox and claim are bound to the registered device and current executor ACL, so shared-profile execution is possible only for an authorized team member. This describes queue tracking only and does not claim that desktop polling, local cookie mutation, or production sync is complete.

## Acceptance

- Wire enqueue schema excludes actor, idempotency, created time, expiry, and unknown fields.
- Enqueue rejects missing/incorrect workspace bindings and missing destination/source ACL before any durable write.
- Creation time and ten-minute expiry are server-generated.
- HTTP returns 202 and stored idempotent replay returns the same job identity.
- OpenAPI has every cookie-job route, query, body, response and status; generated DTOs include those schemas.
- Device inbox lists only jobs bound to that device and currently permitted to its account; claim rechecks destination create and copy-source read ACL.
- Job lookup does not reveal whether another device owns a guessed job ID.
- Targeted API tests and contract/web generation gates pass.

## Boundary

No cookie values are sent to the API. Native CookieAuthority mutation, desktop inbox polling/retry, stage handshake, runtime stop/lease proof, and Windows/macOS execution evidence remain separate release blockers.

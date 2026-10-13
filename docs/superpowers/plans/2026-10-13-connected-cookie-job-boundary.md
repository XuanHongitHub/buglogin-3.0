# Connected Cookie Job Boundary Plan

- [x] Inspect the design-only cookie contract, HTTP routes, device auth, job persistence and profile ACL helpers.
- [x] Separate public enqueue DTO from durable job input; derive actor, idempotency and expiry on the server.
- [x] Enforce destination and independent source profile authorization before persistence.
- [x] Return accepted status and add accurate OpenAPI routes/schemas.
- [x] Add router/contract regression tests and run targeted gates.
- [x] Update readiness to distinguish queue API from missing native device dispatch and production sync proof.
- [x] Add device-scoped inbox listing with current destination/source ACL filtering and claim-time reauthorization.
- [x] Prove a different team member's device can execute only after its own profile ACL is granted; conceal jobs owned by another device.
- [x] Add typed desktop inbox/claim/ack/complete transport methods with device bearer auth, opaque job ID validation, and metadata-only receipts.

## Evidence

- `pnpm test:contract` — 18/18 pass; route inventory now leaves only the two migration-control methods deferred.
- `pnpm check:api` — generated frontend API types are current.
- `CARGO_TARGET_DIR=G:\buglogin-target cargo test --locked -p buglogin-api --test http_cookie_jobs --test openapi_coverage` — 6/6 pass.
- `CARGO_TARGET_DIR=G:\buglogin-target cargo test --locked -p buglogin-desktop-app cookie_mutations_fail_before_any_remote_request` — 1/1 pass after hardening the desktop adapter.
- `CARGO_TARGET_DIR=G:\buglogin-target cargo test --locked -p buglogin-api --test http_cookie_jobs` — 3/3 pass, including requester A → authorized executor B flow and denied-before-grant behavior.
- `CARGO_TARGET_DIR=G:\buglogin-target cargo test --locked -p buglogin-api connected_cookie_jobs` — 5/5 pass, including device inbox isolation and cross-device copy fail-closed behavior.
- `CARGO_TARGET_DIR=G:\buglogin-target cargo test --locked -p buglogin-desktop-app connected_cookie_transport_uses_device_bearer_and_metadata_receipt_only` — 1/1 pass, including exact REST paths, device bearer, redacted receipt fields, and job path validation.
- `pnpm test:contract` — 19/19 pass; generated-source check passes; canonical OpenAPI includes the shared `WorkspaceId` parameter used by inbox.
- Enqueue derives actor, idempotency scope and ten-minute expiry from authenticated server context; source and destination authorization are checked independently.
- `202 accepted` is queue persistence only. Native desktop dispatch, staging handshake, sync reconciliation and hosted staging proof remain incomplete.
- The device inbox filters by the authenticated device and current executor ACL; claim repeats authorization to handle grant revocation between polling and execution. Device-scoped lookup returns not-found for another device's job ID.
- Production desktop adapter now rejects `cookie.stage`, `cookie.import`, and `cookie.copy` before HTTP serialization/transport; a regression test uses an unreachable endpoint and a cookie-value sentinel to prove fail-closed behavior.
- Dispatcher is still blocked on actual native mutation: `wayfern::materialize` rejects plaintext, and `materialize_encrypted` only accepts Chromium `v10/v11` ciphertext. The current `CookieAuthority` is not a substitute for writing the real profile cookie database.

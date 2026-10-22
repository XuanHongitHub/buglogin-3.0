# Desktop Control REST Parity Plan

Spec: docs/superpowers/specs/2026-10-14-desktop-control-rest-parity-design.md.

## Scope and execution

- [x] Map organization and workspace read/write commands to canonical REST routes and wire bodies.
- [x] Map team CRUD, workspace-member lifecycle, and resource inventory/register commands.
- [x] Add loopback route/method/body/auth/idempotency/response tests, including list-page normalization.
- [x] Run desktop library, API contract, generator, format, and diff gates.

## Follow-up implementation — 2026-10-15

- [x] Expose member get/invite/activate/suspend/role-update through the Tauri command handler and typed desktop bridge; the Rust REST routes were already present.
- [x] Add all member lifecycle command IDs to the adapter allowlist and make local mode fail closed for each hosted-authority operation.
- [x] Expose registry-authorized organization/workspace CRUD, team read/create/update, resource read/register, account read, profile read/delete, profile update/proxy assignment, and key-rotation commands through native Tauri handlers and the typed bridge.
- [x] Implement the `events.stream` desktop subscription using the authenticated Rust-held session; Tauri owns the bearer and the webview receives only scoped event payloads. The command refreshes auth before opening the stream; verification and reconnect/expiry E2E remain deferred.
- [x] Ensure a terminated SSE worker removes its subscription entry, including early exits caused by oversized frames; explicit stop remains idempotent.
- [x] Stop reconnecting after non-retryable API errors (including revoked workspace access or authentication); retry only errors the API marks retryable.
- [x] Restore the canonical REST mapping for `operation.get` (`GET /v1/operations/{id}`), which was present in the command registry/API but fell through to the unsupported-command 501 path in production desktop transport. Loopback route/auth/result regression source added; consolidated verification remains deferred.
- [ ] Run the deferred consolidated verification pass after implementation is complete, per current user instruction.

## Boundaries

Do not add domain decisions in the desktop client, alter the canonical API/permission model, or enable local fallbacks. Auth, profile launch/stop, proxy check, events, cookies, and migration retain their specialized paths and stop conditions.

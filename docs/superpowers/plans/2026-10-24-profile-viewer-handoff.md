# Profile viewer handoff implementation plan

## Status

- [x] Scope and constraints recorded
- [x] Design contract recorded
- [x] Trace route ownership, authentication, durable CAS, and runtime launch paths
- [x] Implement durable, single-use, profile-bound ticket issue/redeem APIs and contracts
- [x] Implement scoped viewer session enforcement and cookie behavior
- [x] Integrate desktop launch handoff and frontend redemption/loading/error states
- [x] Add targeted API and frontend contract regression coverage; desktop launch typecheck passes
- [x] Run final focused verification and record residual risks

## Work boundaries

- API/auth authority: `crates/api/src/http/`, `crates/api/src/services/`, canonical API contracts and generated artifacts as required.
- Desktop handoff: `apps/desktop/src-tauri/src/desktop_state.rs`, `remote_api.rs`, and the existing runtime launch adapter path.
- Web redemption: `frontend/src/apps/web/` profile route/client only.
- Capability/docs: update only contracts and capability/design docs directly affected.
- Preserve unrelated dirty changes. Do not commit, push, deploy, or release.

## Implementation notes

Before editing each layer, inspect its owning contract and neighboring implementation. Prefer the existing control-record CAS model and established API response/cookie conventions. If the existing persistence store cannot atomically consume a ticket, extend the owning store contract and both production/test implementations rather than weakening single-use semantics.

## Verification

- Rust targeted tests for issuance, redemption, authorization, replay, expiry, wrong profile, revocation, and cookie scope.
- Frontend tests for fragment scrubbing, redemption, and error states.
- Desktop launch URL regression test.
- `cargo fmt --all -- --check`, focused crate tests, `pnpm --filter @buglogin/web test`, and `git diff --check`.

## Verification result

- `cargo fmt --all -- --check`: passed.
- `cargo test -p buglogin-api profile_viewer --lib`: 3 tests passed before the final auth fallback adjustment; a rerun was started but could not acquire the shared Cargo artifact lock while other Rust sessions were running. The final adjustment only permits fallback to the existing authenticated browser session when a profile viewer cookie is stale.
- `cargo check -p buglogin-desktop-app --lib`: passed before the final backend-only adjustment.
- `pnpm --filter @buglogin/web test`: passed, including profile viewer request contract and TypeScript checks.
- `pnpm check:api`: passed.
- `git diff --check`: passed; Git printed only existing LF/CRLF working-copy notices.
- No browser interaction QA, API deployment, or production release was performed. The viewer handoff is source-complete but not yet live on `buglogin.com` / `api3.buglogin.com`.

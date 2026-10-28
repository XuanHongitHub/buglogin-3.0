# BugLab ID SSO implementation plan — 3.0.19

## Status (updated 2026-10-28)

Provider login and exchange passed using the documented QA identity. The provider's returned state is carried in the registered callback URI, and the desktop persists a matching, expiring nonce in the OS credential vault so a callback process can validate it. Stable 3.0.21 is published. Root cause for isolated QA: `BUGLOGIN_INSTANCE_ID` intentionally disables the global single-instance guard, so the OS-started callback process has no isolated identity and cannot find the original instance's pending nonce. A state-keyed OS-vault route and bounded loopback HMAC handoff are implemented locally; isolated SSO fails before opening the browser unless its callback listener is ready. Desktop Rust compile and four focused callback tests pass, including cross-instance loopback transfer and one-time route consumption against a shared test vault. Packaged Windows A/B routing, final BugLogin session issuance, macOS callback, and logout integration still need verification.

## Ordered work

1. **Provider contract** — real login, callback-state return, exchange, active status, and BugLogin mapping were verified on 2026-10-27. Continue tracking remaining provider questions in the design spec.
2. **Identity and protocol contract** — SSO start/complete API and OpenAPI schemas are implemented; identity mapping remains role-neutral and ambiguity fails closed.
3. **Backend broker** — implemented: expiring single-use state, bounded HTTPS exchange, active-status/app checks, subject mapping, session issue, and redacted audit.
4. **Desktop flow** — system-browser launch, strict custom-scheme callback parsing, and device-session persistence use the existing OS credential vault. Native callbacks are retained until the frontend drains them. Cross-instance callback handoff is implemented locally with a hashed state route in the OS vault, loopback HMAC envelope, strict size/query validation, one-use route removal, isolated-mode fail-fast, and pending-state cleanup if routing/browser launch fails.
5. **Rollout gate** — implemented: API requires `BUGLAB_SSO_ENABLED=true`, app code, and callback URI; the production BugLogin API is configured for BugLab ID.
6. **Logout** — BugLogin session revocation uses existing logout. BugLab ID logout redirect is pending exact allowlisted return URI/provider behavior.
7. **Verification and release** — provider login/exchange and focused nonce tests pass; stable 3.0.21 is published. Packaged Windows/macOS callback, complete session issuance, logout integration, and installed-user acceptance still require verification. Isolated QA callback routing requires its own packaged A/B acceptance before it can be called supported.

## Repository gates

- Identity owner: `WP-13` / IO-CONTROL.
- REST/OpenAPI contract integration: `WP-19` / IO-AUTO.
- Desktop UI and callback interaction: `WP-21` / IO-UI, with `WP-44` OS acceptance.
- Use the corresponding `C-API`, `C-SECURITY`, `C-OPENAPI`, `C-DESKTOP`, `C-E2E-WIN`, and `C-E2E-MAC` gates from `docs/agents/work-packet-ownership.md`.
- Production implementation remains subject to the Gate 0/readiness status in `docs/readiness/implementation-readiness.md` and cannot be called production-ready based on local mocks.

## Rollback

SSO is additive and provider-configurable. Disable the BugLab provider route/feature flag, preserve existing local account identities/sessions per the existing revocation policy, and keep password/device sign-in available. No tenant data migration is required; any persisted provider-subject links require a reversible, versioned identity-record migration and pre/post snapshot evidence.

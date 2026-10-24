# BugLab ID SSO implementation plan — 3.0.4

## Status

Backend broker and desktop callback flow are implemented and compile; no production provider call has been made. SSO is off unless explicitly enabled. Provider-side app/callback/state configuration is not documented in the supplied guide and remains a real external dependency.

## Ordered work

1. **Provider contract (blocking external input)** — confirm the seven items in `docs/superpowers/specs/2026-10-23-buglab-id-sso-design.md`. The guide omits callback/state support and the production app code.
2. **Identity and protocol contract** — SSO start/complete API and OpenAPI schemas are implemented; identity mapping remains role-neutral and ambiguity fails closed.
3. **Backend broker** — implemented: expiring single-use state, bounded HTTPS exchange, active-status/app checks, subject mapping, session issue, and redacted audit.
4. **Desktop flow** — implemented: system-browser launch, strict custom-scheme callback parsing, and device-session persistence through the existing OS credential vault.
5. **Rollout gate** — implemented: API requires `BUGLAB_SSO_ENABLED=true`, app code, and callback URI. Keep disabled until provider contract is confirmed.
6. **Logout** — BugLogin session revocation uses existing logout. BugLab ID logout redirect is pending exact allowlisted return URI/provider behavior.
7. **Verification and 3.0.4 release** — Rust check and frontend typecheck pass. Tests, generated contract parity, Windows/macOS callback smoke, logout integration, and real provider verification remain outstanding.

## Repository gates

- Identity owner: `WP-13` / IO-CONTROL.
- REST/OpenAPI contract integration: `WP-19` / IO-AUTO.
- Desktop UI and callback interaction: `WP-21` / IO-UI, with `WP-44` OS acceptance.
- Use the corresponding `C-API`, `C-SECURITY`, `C-OPENAPI`, `C-DESKTOP`, `C-E2E-WIN`, and `C-E2E-MAC` gates from `docs/agents/work-packet-ownership.md`.
- Production implementation remains subject to the Gate 0/readiness status in `docs/readiness/implementation-readiness.md` and cannot be called production-ready based on local mocks.

## Rollback

SSO is additive and provider-configurable. Disable the BugLab provider route/feature flag, preserve existing local account identities/sessions per the existing revocation policy, and keep password/device sign-in available. No tenant data migration is required; any persisted provider-subject links require a reversible, versioned identity-record migration and pre/post snapshot evidence.

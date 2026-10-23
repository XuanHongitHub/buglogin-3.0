# BugLab Federated Desktop Login Plan

Spec: `docs/superpowers/specs/2026-10-23-buglab-federated-desktop-login-design.md`.
OpenSpec: `openspec/changes/buglab-desktop-federated-login/`.

Status: **design only; no implementation authorized until the listed contracts and UI reference are approved.**

## Contract and ownership

- [ ] Define the provider-neutral, versioned BugLogin broker contract; confirm BugLab protocol, issuer, registration, subject stability, MFA assurance, callback, logout/revocation, and account-disable semantics.
- [ ] Approve identity linking and unlinked-account behavior; never link by email alone.
- [ ] Assign IO-CONTROL, IO-AUTO, IO-UI, and IO-RELEASE path owners; update the ownership manifest.
- [ ] Approve ACC-02 provider option reference with enabled, disabled, offline, loading, cancel, denied, and success states.
- [ ] Validate and implement server-only identity-provider API environment variables and public provider metadata. Missing config remains disabled; compatible endpoint changes stay API-only.

## API broker and desktop session

- [ ] Add provider discovery and authorization transaction routes with strict schemas, idempotency/replay behavior, audit, and rate limits.
- [ ] Add BugLab OIDC broker validation for issuer, audience, signature, nonce, exact redirect, and MFA policy.
- [ ] Add one-time short-lived callback code bound to desktop PKCE, state, and device context.
- [ ] Exchange the code through Rust into the existing canonical BugLogin device session and vault.
- [ ] Keep provider credentials and tokens out of frontend JavaScript, deep-link URLs, logs, and handoff files.

## UI, OS, and verification

- [ ] Add the BugLab option only when the API advertises it; preserve password login and show real disabled/error state.
- [ ] Add Tauri callback validation and cancellation with stale, duplicate, wrong-host, wrong-scheme, and replay rejection.
- [ ] Test API identity mapping, token issuance, audit, session refresh/revocation, and REST/Tauri contract compatibility.
- [ ] Complete approved visual/interaction/accessibility QA for ACC-02.
- [ ] Verify native browser redirects, deep-link callback, install/update, and retained data on Windows and macOS.
- [ ] Deploy and parity-verify API config/routes through Packet A; validate exact CORS origins only if a browser client needs cross-origin fetches.
- [ ] Build immutable `v3.0.4` only after API and OS evidence gates pass; do not change stable feed as part of this proposal.

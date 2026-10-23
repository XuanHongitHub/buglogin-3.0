# Tasks

- [ ] Define a provider-neutral, versioned BugLogin broker contract; confirm BugLab protocol, issuer/client registration, redirect allowlist, stable subject, MFA assurance, and logout/revocation contract with the identity owner.
- [ ] Approve canonical identity linking and unlinked-account behavior; prohibit email-only identity linking in service tests.
- [ ] Assign exact IO-CONTROL, IO-AUTO, IO-UI, and IO-RELEASE write scopes and update the work-packet manifest.
- [ ] Approve the ACC-02 desktop sign-in reference package, including provider enabled/disabled, loading, offline, denied, callback failure, and success states.
- [ ] Add fail-closed API configuration and a non-secret provider metadata capability response; keep OIDC client secrets server-only and allow compatible endpoint changes without desktop changes.
- [ ] Add the API-brokered authorization-code + PKCE flow with state/nonce, one-time code, short expiry, replay denial, rate limits, audit, and canonical device-session exchange.
- [ ] Add typed Tauri commands for provider discovery, flow start, callback validation, code exchange, and cancellation; keep credentials and tokens in Rust/OS vault paths.
- [ ] Add the provider option to ACC-02 using approved shared controls and localized text; preserve password login and expose errors from real API state.
- [ ] Handle only the registered BugLogin callback scheme/host/path; validate state and PKCE, reject duplicate/stale callbacks, redact callback data, and test Windows/macOS registration.
- [ ] Decide CORS only for actual browser-based clients; for the native desktop flow, verify system-browser navigation and Rust HTTPS exchange without broadening CORS.
- [ ] Add identity, API/OpenAPI, Tauri, UI, audit, security, and Windows/macOS E2E coverage, including callback replay, account-link mismatch, disabled provider, cancellation, and session revocation.
- [ ] Deploy API configuration/routes through Packet A's release process; verify exact CORS allowlist if web cross-origin calls are introduced.
- [ ] Build and sign an immutable 3.0.4 candidate; prove clean install and update on Windows/macOS before making any release claim.

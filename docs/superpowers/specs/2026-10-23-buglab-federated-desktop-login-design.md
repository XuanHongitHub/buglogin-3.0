# BugLab Federated Desktop Login

Status: **proposal; implementation blocked on identity contract, packet ownership, and approved ACC-02 reference.**

## User outcome

The desktop offers a BugLab login option beside existing BugLogin credentials. The installed app discovers whether the provider is enabled from the canonical BugLogin API. Provider configuration, including authorization endpoints and public client metadata, is API-owned and environment-driven. Once API configuration is deployed, users can authenticate without a new desktop build when the API implements the stable broker contract and the provider uses a supported protocol.

## Authority and flow

1. Rust asks the pinned BugLogin API for public auth-provider metadata.
2. If BugLab is enabled, the UI offers the provider option.
3. Rust creates a PKCE verifier/challenge and state, then asks the BugLogin broker to start a transaction.
4. The app opens the returned allowlisted authorization URL in the operating-system browser. API configuration may change provider hosts/endpoints without changing the desktop, while the desktop stays pinned to BugLogin as its broker.
5. BugLogin API brokers OIDC with BugLab, validates issuer, audience, signature, nonce, MFA policy, and callback state, and resolves verified `(issuer, subject)` to a canonical BugLogin account.
6. The broker redirects a short-lived one-time code to the registered `buglogin://auth/callback` route. It never places access/refresh tokens or account IDs in that URI.
7. Tauri validates the callback shape and state; Rust exchanges the code plus PKCE verifier with BugLogin API.
8. The API returns the existing device-bound BugLogin session. Rust persists it through existing OS-vault/session authority; frontend receives only the normal auth status.

The desktop does not trust BugLab tokens as BugLogin API credentials. An unlinked identity is denied or follows a separately approved linking flow; equal email addresses do not prove identity ownership.

## CORS

No BugLab CORS grant is needed for this native flow: the OS browser performs top-level HTTPS navigation and Tauri Rust performs API HTTPS requests. Do not move federation token exchange into WebView JavaScript to work around CORS. If a BugLab web app later calls BugLogin API from JavaScript, configure only that exact HTTPS origin under the API's existing credentialed CORS rules; wildcard origins are invalid with credentials.

## API-host configuration

The API owner should define and validate server-only environment configuration for provider enablement, protocol/issuer/discovery, client ID, client secret, broker callback, allowed scopes, and display metadata. Confirm the exact variable names and BugLab endpoints with its identity owner before API activation. Missing or invalid values disable the provider and fail closed. The API returns no client secret to desktop clients. Keep a provider-neutral, versioned broker contract so endpoint changes can be API-only; protocol changes still need compatible API implementation.

Desktop remains pinned to `https://api3.buglogin.com`; it must not load provider endpoints from a local environment variable or accept host overrides from callback data. The app can wait for server rollout only if provider discovery and auth broker routes are deployed with compatible API behavior.

## UI and ownership gates

The affected screen is ACC-02 Sign in (P0). `docs/design/approved-reference-checklist.md` has no approved ACC-02 reference package, so UI edits are blocked until one is approved. Required owners are IO-CONTROL for account mapping/session issuance, IO-AUTO for routes/OpenAPI/config, IO-UI for login presentation and Tauri command wiring, and IO-RELEASE for deep-link registration and OS verification. Update packet ownership before parallel implementation.

## Security and test gates

- Enforce issuer/audience/signature/nonce, PKCE, state, exact redirect URI, one-time code, short expiry, replay denial, rate limits, and correlation-safe audit.
- Never log OIDC tokens, codes, PKCE verifiers, secrets, or callback URLs containing codes.
- Test disabled/invalid configuration, cancellation, stale callback, replay, state mismatch, account-link mismatch, disabled account, MFA policy, session refresh/revocation, and parity with password-login DesktopAuthStatus.
- Exercise Tauri deep links and installed-app callback behavior on Windows and macOS before release.
- Do not promote stable or claim production readiness without API parity and OS install/update evidence.

## Open decisions

- BugLab protocol, issuer, discovery URL, client registration, redirect allowlist, stable subject guarantees, MFA assurance, logout, and account-disable semantics.
- Whether unlinked BugLab identities are denied pending an admin link or may use an explicit canonical account provisioning path.
- The approved ACC-02 provider-option design and exact localized copy.
- API and desktop packet owners and exact path assignments.

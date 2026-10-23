# BugLab Federated Desktop Login

## Why

BugLogin desktop needs a second sign-in option that delegates authentication to the BugLab central identity system and then establishes the existing BugLogin account/device session. The installed app must be able to expose the option from server configuration after release, without rebuilding the desktop when the BugLab provider is enabled.

## Change

Add a generic API-advertised federated provider and a native-app authorization-code flow, initially labeled BugLab. BugLogin API is the federation broker: it sends the user through the configured identity provider, maps the verified `(issuer, subject)` pair to the canonical BugLogin account, and returns a short-lived, single-use code to the BugLogin desktop deep link. The desktop exchanges the code and a PKCE verifier over its Rust HTTPS transport for the existing device-bound BugLogin session. Provider endpoints, issuer, client ID, scopes, and display metadata come from API configuration; client secrets stay on the API host.

The login option is available only when the BugLogin API advertises BugLab as enabled. Keep direct BugLogin password login unchanged. Do not put access tokens, refresh tokens, provider secrets, or account IDs in a deep-link URL. Do not auto-link identities by email alone.

Use the system browser for federation and native Rust transport for provider discovery, code exchange, and session persistence. This avoids WebView cross-origin fetches; CORS is not needed for this native flow. If a future web client calls the API cross-origin, allow only its exact HTTPS origin with the existing credentialed CORS policy; never use wildcard origins with credentials.

## Ownership and release boundary

This is a multi-owner change: identity authority and mapping (IO-CONTROL), API routes/OpenAPI/configuration (IO-AUTO), desktop UI and Tauri auth commands (IO-UI), and deep-link/OS handling (IO-RELEASE). Assign exact packet paths before implementation. Do not include API changes in Packet B's release-workflow scope. The 3.0.3 release candidate remains independent; this change targets a future 3.0.4 only after its contract, UI reference, API deployment, and OS evidence gates pass.

## Compatibility and deployment

The provider is disabled by default and fails closed when identity-provider configuration is absent or invalid. The API exposes only validated, non-secret provider availability and authorization metadata to the desktop. OIDC client credentials remain on the API host. The desktop continues to call the pinned BugLogin API origin; it never accepts an arbitrary provider/API URL from a local environment variable or deep link. Provider endpoints can change without a desktop rebuild while the API preserves the versioned broker contract.

Provider linking, account provisioning, MFA policy, logout, account-disabled behavior, and missing identity handling must use the existing canonical account and device-session authority. No BugLab token is accepted as a BugLogin API bearer token.

## Risks and unresolved decisions

- Confirm the BugLab protocol, issuer, discovery support, client registration, redirect URI, subject stability, MFA assurance, and logout/session revocation semantics with the BugLab owner. Keep the desktop contract provider-neutral.
- Decide whether an unlinked BugLab identity is denied pending an administrator-managed link or may enter an explicit account-provisioning flow. Email-only auto-linking is forbidden.
- Confirm a production-approved ACC-02 sign-in reference package. Current reference checklist does not contain one, so desktop UI implementation is blocked.
- Assign API, identity, UI, and OS packet owners before changing their owned files.
- Production API is currently source-out-of-sync; a 3.0.4 client cannot rely on the live server until the required routes/configuration are deployed and parity-verified.

## Out of scope

- Replacing or removing BugLogin password login.
- Browser/WebView CORS-based OAuth token delivery.
- Storing BugLab credentials or provider tokens in the desktop.
- Stable updater feed promotion or production readiness claims.

# BugLab ID SSO for BugLogin 3.0.4

## Status

Backend broker, identity mapping, desktop system-browser launch, and native callback consumption are implemented and compile. SSO remains disabled by default; the provider guide does not confirm the required state round-trip or native callback registration, so production enablement and release acceptance are blocked on those external facts.

## Source

`D:\Download\HUONG_DAN_TICH_HOP_BUGLAB_ID_SSO.md` describes `https://id.buglab.vn`, a five-minute single-use token, `POST /api/v1/sso/exchange`, a returned user record, optional account auto-provisioning, and an IDP logout redirect.

## Current BugLogin boundary

BugLogin currently issues its own device-bound sessions. Its production device authorization contract explicitly lists organization SSO as a separate identity wave. The desktop already registers the `buglogin` deep-link scheme, but the supplied provider guide only demonstrates an HTTPS callback; it does not establish native callback registration, state handling, or the BugLogin app mapping value.

## Proposed flow

1. Desktop asks BugLogin API to begin an SSO transaction. API creates a random, expiring, single-use state transaction and returns the approved BugLab ID login URL with the BugLogin app code and exact callback.
2. Desktop opens the system browser. BugLab ID authenticates the person and returns a one-time token plus state to the registered callback. The supplied guide documents only `token`; state return and callback registration are unverified.
3. Desktop forwards callback data and the transaction handle to BugLogin API. The API verifies state/expiry/single-use and calls BugLab ID exchange over TLS.
4. API validates active status, stable provider subject, canonical verified email, and explicit BugLogin account mapping. API links only an unambiguous account; any new account is provisioned without tenant grants.
5. API issues BugLogin's normal device-bound session. Logout revokes that session and follows the approved IDP logout policy.

The current implementation expects `buglogin://sso/callback?token=...&state=...`. `BUGLAB_SSO_ENABLED` must be exactly `true` in addition to valid app-code and callback configuration before the API enables SSO. Keep it unset/false until BugLab ID confirms this native callback and returns state unchanged. Do not put the raw BugLab token into logs, analytics, or persistent desktop storage.

## Mapping and access policy

- Provider subject is the stable BugLab `user_id`; email is a contact/verified attribute, not the sole durable key.
- `all_mappings` must contain an explicit BugLogin app entry, or the provider must confirm the correct equivalent field. Do not infer it from `old_account` for another app.
- Provider `status=active` is required. Suspended BugLogin accounts remain denied even when BugLab ID is active.
- SSO establishes identity only. Organization/workspace membership, role, profile grants, and platform-admin status remain controlled by BugLogin's existing authorization model.
- Ambiguous identity mappings fail closed and are resolved through a separate audited linking operation.

## Required provider answers

1. What exact `app` code is registered for BugLogin?
2. Which exact redirect URI can be registered for Windows and macOS? Are custom URI schemes accepted, or is an HTTPS handoff required?
3. Does `/sso/login` accept and return an opaque `state` unchanged? If not, what provider-supported CSRF binding is available?
4. Is `user_id` stable and immutable? Which key in `all_mappings` is BugLogin's canonical legacy-account mapping?
5. What is the authoritative active/suspended status contract, and are email claims verified?
6. Which logout redirect URIs are allowlisted, and does logout end the global BugLab ID session?
7. Is there a sandbox/test tenant and test account for end-to-end verification?

## Acceptance

Use the acceptance scenarios in `openspec/changes/buglab-id-sso/specs/identity-sso/identity-sso_delta.md`. Release evidence must include real provider login, correct account mapping, no implicit role grant, replay/state denial, logout, and Windows/macOS callback smoke. A mocked exchange is not production evidence.

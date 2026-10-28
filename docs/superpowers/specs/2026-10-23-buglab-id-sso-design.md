# BugLab ID SSO for BugLogin 3.0.4

## Status (updated 2026-10-27)

The BugLab provider login, callback, and token exchange have been exercised with the documented QA account. The provider returns `state` and `token` to the registered custom-scheme callback when the nonce is also embedded in `redirect_uri`; a top-level login query parameter alone is not preserved. The API builds the callback URI with that nonce, and the desktop checks it against the unexpired sign-in request before exchanging the token. The API returned an active BugLogin mapping during the provider smoke test.

This verifies the provider path and exchange contract. A full packaged Windows/macOS callback and BugLogin session issuance smoke is still required before claiming release acceptance.

## Source

`D:\Download\HUONG_DAN_TICH_HOP_BUGLAB_ID_SSO.md` describes `https://id.buglab.vn`, a five-minute single-use token, `POST /api/v1/sso/exchange`, a returned user record, optional account auto-provisioning, and an IDP logout redirect.

## Current BugLogin boundary

BugLogin currently issues its own device-bound sessions. Its production device authorization contract explicitly lists organization SSO as a separate identity wave. The desktop already registers the `buglogin` deep-link scheme, but the supplied provider guide only demonstrates an HTTPS callback; it does not establish native callback registration, state handling, or the BugLogin app mapping value.

## Proposed flow

1. Desktop asks BugLogin API to begin an SSO transaction. API creates a random, expiring, single-use state transaction and returns the approved BugLab ID login URL with the BugLogin app code and exact callback.
2. Desktop opens the system browser. BugLab ID authenticates the person and returns a one-time token plus state to the registered callback. The nonce is included in both the login query and callback URI because the provider preserves the callback query.
3. Desktop forwards callback data and the transaction handle to BugLogin API. The API verifies state/expiry/single-use and calls BugLab ID exchange over TLS.
4. API validates active status and the stable provider subject, then links only an already-provisioned BugLogin account. BugLab email may change after the stable subject is linked; that change must not remap the provider identity to another BugLogin account. An unknown BugLab identity does not create a login-only account with no workspace access. Workspace admins provision the BugLogin account and grant its workspace role before the user signs in.
5. API issues BugLogin's normal device-bound session. Logout revokes that session and follows the approved IDP logout policy.

### Native callback routing across isolated desktop instances

When QA or another explicitly isolated desktop instance is running, it must keep its own device identity and credential-vault namespace. A protocol callback may start an untagged process, so the callback process must route the URL to the instance that owns the matching SSO `state` instead of attempting token exchange in the wrong instance. Each running instance advertises a short-lived route keyed by a domain-separated hash of the opaque SSO state in the OS credential vault. The route contains only the loopback listener port, an ephemeral HMAC key, and expiry. The callback URL is forwarded over `127.0.0.1` with an HMAC-authenticated, size-bounded message; the receiving instance validates the callback shape, route, signature, and expiry before dispatching it to the existing single-use SSO completion path. Raw callback URLs/tokens must not be written to disk, logs, analytics, or the route record. If route setup fails for an isolated instance, fail SSO start before opening the browser. The ordinary single-instance path remains compatible and may consume its own callback locally.

The implementation expects `buglogin://sso/callback?token=...&state=...`. Production API configuration enables the provider with `BUGLAB_SSO_ENABLED=true`, a valid app code, and `buglogin://sso/callback`. The desktop binds callbacks to the nonce from the current, unexpired start request. Do not put the raw BugLab token into logs, analytics, or persistent desktop storage.

## Mapping and access policy

- Provider subject is the stable BugLab `user_id`; email is a contact/verified attribute, not the sole durable key.
- `all_mappings` must contain an explicit BugLogin app entry, or the provider must confirm the correct equivalent field. Do not infer it from `old_account` for another app.
- Provider `status=active` is required. Suspended BugLogin accounts remain denied even when BugLab ID is active.
- BugLab SSO may provision a previously unseen identity into the BugMedia organization and its explicitly named common workspace (or the sole workspace when there is only one). The new account is verified by the successful provider exchange and receives only the ordinary `Member` workspace role; it receives no platform-admin status, team assignment, or profile grants.
- If BugMedia or an unambiguous common workspace is not configured, provisioning fails before creating any identity or account. Ambiguous email/legacy-account mappings to existing accounts still fail closed. A changed email on an existing stable subject is not ambiguous and must not trigger administrator review.

## Remaining provider questions

1. What exact `app` code is registered for BugLogin?
2. Which exact redirect URI can be registered for Windows and macOS, and are custom URI schemes supported consistently on both platforms?
3. Is callback query preservation a supported provider contract, and what provider-supported CSRF binding applies if it changes?
4. Is `user_id` stable and immutable? Which key in `all_mappings` is BugLogin's canonical legacy-account mapping?
5. What is the authoritative active/suspended status contract, and are email claims verified?
6. Which logout redirect URIs are allowlisted, and does logout end the global BugLab ID session?
7. Is there a dedicated sandbox/test tenant and test account for ongoing end-to-end verification?

## Acceptance

Use the acceptance scenarios in `openspec/changes/buglab-id-sso/specs/identity-sso/identity-sso_delta.md`. Release evidence must include real provider login, correct account mapping, no implicit role grant, replay/state denial, logout, and Windows/macOS callback smoke. A mocked exchange is not production evidence.

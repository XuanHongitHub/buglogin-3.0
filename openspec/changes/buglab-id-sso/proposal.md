# Add BugLab ID SSO to BugLogin 3.0.4

## Why

BugLogin 3.0.4 must support the centralized BugLab ID sign-in flow described in `D:\Download\HUONG_DAN_TICH_HOP_BUGLAB_ID_SSO.md`. BugLogin currently has local account/device authentication and explicitly defers organization SSO.

## Scope

- Add BugLab ID as an optional authentication provider for the BugLogin desktop app.
- Keep existing BugLogin authentication available during rollout and recovery.
- Authenticate and exchange the one-time BugLab token only through trusted backend code; the desktop must never treat callback user fields as authoritative.
- Link a BugLab subject to an existing BugLogin account safely. Any just-in-time provisioned account receives no organization, workspace, role, or profile access until separately granted.
- Revoke the BugLogin session on sign-out and support the documented BugLab ID sign-out redirect.
- Include Windows and macOS callback, cancel, expiry, replay, and restart behavior in 3.0.4 acceptance.

## Security decisions

- Bind the login attempt to an unpredictable, short-lived, single-use state value and an exact registered callback. The supplied BugLab guide does not document state support; this must be confirmed with the BugLab ID owner before enabling SSO.
- Treat `user_id` as the provider subject. Match legacy accounts only through an explicit BugLogin app mapping returned by the trusted exchange response; do not use arbitrary `old_account` values as email or grant authority.
- Require an active provider identity. Provider identity and verified email do not grant BugLogin tenant membership or roles.
- Validate exact issuer/endpoint, TLS, response size/schema, timeout, token expiry/replay, and canonical email. Never log the one-time token or exchange response secrets.
- Do not silently replace an existing verified BugLogin identity when email or legacy mapping is ambiguous; require an audited account-link operation.

## External dependency / gate

Before implementation can be accepted, BugLab ID must confirm the BugLogin app code, exact registered callback URI(s) for Windows/macOS, `state` round-trip support, the stable meaning of `user_id`, the BugLogin value in `all_mappings`, and logout redirect allowlisting. The supplied guide alone does not establish these provider-side settings or a native-app callback contract.

The backend and desktop code are implemented behind an explicit opt-in. The API enables the provider only when `BUGLAB_SSO_ENABLED=true`, `BUGLAB_SSO_APP_CODE`, and `BUGLAB_SSO_CALLBACK_URI` are configured. Leave the enable flag off until state round-trip and callback registration are confirmed.

## Compatibility

Existing password/device login, refresh, and logout APIs remain compatible. SSO is additive. No account receives additional permissions as a result of this change. The API/OpenAPI contract and generated clients must remain in parity.

## Acceptance

- Valid active BugLab identity signs in on Windows and macOS and receives a normal BugLogin session bound to the mapped account.
- Missing/invalid state, wrong callback, expired/replayed token, inactive identity, exchange outage, malformed response, ambiguous mapping, and unapproved email all fail closed without creating a privileged account/session.
- Sign-out revokes the BugLogin session and performs the approved BugLab ID logout flow.
- Account linking/provisioning is idempotent, audited, and role-neutral; tenant access is assigned only through existing membership/grant flows.
- Tests cover API exchange, identity mapping, session issuance/revocation, replay races, callback handling, redaction, role matrix, Windows/macOS smoke, and existing auth regression suites.
- Production readiness requires a successful end-to-end run against BugLab ID's approved test/production integration and recorded evidence; mocked exchange tests alone are insufficient.

# Profile viewer handoff design

## Problem

The browser runtime opens `https://buglogin.com/profileId={profileId}` in an isolated profile. That browser does not have the desktop app's BugLogin web session, so the profile detail page is intercepted by the account-wide web auth gate.

## Goal

Allow the desktop app to hand off read-only access to exactly one profile detail page without putting account credentials in the browser URL or granting account-wide web access.

## Security and behavior contract

- The desktop app requests a handoff only after its existing authenticated profile read succeeds.
- The API issues a cryptographically random, short-lived, single-use ticket bound to the authenticated account and one profile.
- Only a hash of the ticket is persisted. Raw ticket values must not enter logs, audit payloads, or durable records.
- The ticket is carried in the URL fragment, which is not sent in HTTP requests or Referer headers. The page removes it from browser history immediately and redeems it in a POST body.
- Redemption creates a short-lived, profile-scoped, read-only session. It must not authenticate `/v1/auth/session` or any account/workspace/admin endpoint.
- Every profile data read rechecks the profile binding and current ACL. Wrong-profile use and revoked access fail closed.
- Ticket redemption is atomic and single-use across API instances. Use the existing durable control persistence and revision/CAS patterns; no process-local in-memory authority.
- The cookie is Secure, HttpOnly, SameSite=Strict, and path-scoped to that profile's API route. It is not returned to JavaScript.
- Existing account-session behavior remains unchanged. Ordinary web routes still require normal login.

## UI flow

1. Desktop launches the runtime with the existing `profileId` route plus a fragment handoff ticket.
2. Profile detail route detects the fragment before querying profile data, removes it using `history.replaceState`, and POSTs it to the profile-scoped redemption endpoint.
3. On redemption success, the page loads profile data through the scoped cookie. Loading, expired/invalid ticket, forbidden, and API error states remain explicit.
4. Refresh works while the scoped session remains valid; expiry returns a clear re-open-profile action.

## API and audit

- Add authenticated ticket-issue and anonymous ticket-redemption operations to the canonical API contract.
- Ticket issuance requires existing profile read authorization.
- Redemption is limited to the ticket's profile and creates a read-only viewer session.
- Audit issuance and redemption using non-sensitive identifiers/state only; never include credentials or ticket values.
- Update capability registry and API docs/contracts.

## Validation

Focused backend tests must cover permission denial, successful issue/redeem, single-use, expiry, wrong-profile replay, current ACL revocation, and profile-only scope. Frontend tests cover fragment scrubbing, redemption, and fail-closed states. Desktop tests verify launch URL carries a fragment ticket and does not alter ordinary page/profile launch behavior. Run the smallest relevant Rust and frontend gates, plus formatting and diff checks.

## Out of scope

- No public unauthenticated profile endpoint.
- No account-wide browser login or persisted account bearer token in Wayfern.
- No production deploy, release, or Git push in this task.

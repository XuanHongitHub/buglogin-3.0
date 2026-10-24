# Identity SSO requirements

## Requirement: BugLab ID authentication

BugLogin SHALL provide an optional BugLab ID SSO sign-in path for the 3.0.4 desktop release while preserving the current authentication path during rollout.

### Scenario: Active mapped identity signs in

- GIVEN a fresh BugLogin SSO transaction bound to an exact registered callback and state
- AND BugLab ID reports an active identity with a stable subject and an unambiguous BugLogin mapping
- WHEN the backend exchanges the one-time token with BugLab ID
- THEN BugLogin creates or resolves the mapped account and issues its normal device-bound session
- AND the session receives only permissions already assigned through BugLogin's membership and grant model

### Scenario: Account is provisioned without tenant grants

- GIVEN an active BugLab identity with no existing BugLogin account
- AND policy permits just-in-time account creation
- WHEN the backend provisions the account
- THEN it creates a verified provider identity linked to the stable BugLab subject
- AND it grants no organization/workspace membership, role, or profile access by default

### Scenario: Callback is invalid or replayed

- GIVEN an SSO transaction with missing/mismatched state, wrong callback, expired token, or a previously consumed token
- WHEN the callback is completed
- THEN the request fails closed, no session is issued, and no privileged membership is created

### Scenario: Identity mapping is ambiguous

- GIVEN the provider subject, verified email, or explicit BugLogin mapping points to conflicting BugLogin accounts
- WHEN the backend resolves the SSO identity
- THEN it refuses automatic linking and requires an audited account-link operation

### Scenario: Provider unavailable

- GIVEN BugLab ID is unavailable or returns an invalid response
- WHEN the backend attempts token exchange
- THEN it returns a stable retryable authentication error, issues no session, and does not log the token or sensitive response fields

### Scenario: User signs out

- GIVEN an authenticated BugLogin session created through SSO
- WHEN the user signs out
- THEN BugLogin revokes its local session and follows the approved BugLab ID logout redirect behavior

## Requirement: Provider boundary

Only the BugLogin backend SHALL exchange BugLab one-time tokens and interpret provider identity claims. The desktop SHALL pass a short-lived callback token and transaction binding to the backend; it SHALL NOT provision accounts or assign roles itself.

## Requirement: Provider contract gate

SSO SHALL remain disabled until the BugLab ID owner confirms the BugLogin app registration, exact callback URIs, state round-trip, stable subject semantics, app-specific account mapping, and logout redirect allowlist.

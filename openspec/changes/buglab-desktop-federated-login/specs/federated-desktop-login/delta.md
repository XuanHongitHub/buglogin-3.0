# Federated Desktop Login Spec Delta

## ADDED Requirements

### Requirement: BugLab provider availability is server-authoritative

The system MUST expose the BugLab-labeled provider to the desktop only when the canonical BugLogin API confirms the provider is enabled and its server configuration is valid. The provider MUST be disabled by default. Provider endpoints and public metadata MUST come from the versioned BugLogin API broker contract. The desktop MUST NOT contain provider client secrets or accept an arbitrary authorization/API origin from its local environment, browser, or deep link. The API MUST support endpoint changes without a desktop rebuild while preserving that broker contract.

#### Scenario: Provider disabled before server configuration

- GIVEN BugLab federation is disabled or required API configuration is missing
- WHEN the desktop fetches the canonical provider list
- THEN the API reports BugLab unavailable without disclosing secret configuration
- AND the desktop does not start a federation flow

#### Scenario: Provider enabled after API configuration

- GIVEN a compatible desktop build is installed
- AND the API has valid BugLab identity-provider configuration and advertises the provider enabled
- WHEN the desktop refreshes provider availability
- THEN it can start BugLab login without a desktop rebuild

### Requirement: Desktop federation uses a brokered code with PKCE

The system MUST use the system browser and a BugLogin API-brokered authorization-code flow. The desktop MUST bind the flow to an unpredictable state, PKCE verifier/challenge, and an OIDC nonce validated by the broker. The callback MUST contain only a short-lived, single-use authorization code and state. The desktop MUST exchange the code over its Rust HTTPS client for the existing device-bound BugLogin session.

#### Scenario: Successful BugLab login

- GIVEN the user has a verified BugLab identity mapped to an active canonical BugLogin account
- WHEN the user completes BugLab authentication in the system browser
- THEN the broker redirects a short-lived one-time code to the registered BugLogin deep link
- AND Rust exchanges it with the matching state and PKCE verifier
- AND the API returns the existing canonical BugLogin device session

#### Scenario: Callback replay or mismatch

- GIVEN a callback code was consumed, expired, has invalid state, or does not match the PKCE verifier
- WHEN the desktop attempts to exchange it
- THEN the API denies the exchange and creates no session
- AND the desktop clears the pending flow state

### Requirement: Federated identity links use stable provider subject

The API MUST resolve federation identity by verified `(issuer, subject)` and MUST NOT silently link accounts by email equality. An unlinked, disabled, or cross-tenant identity MUST follow the explicitly approved account-link/provisioning policy and produce no BugLogin session until that policy succeeds.

#### Scenario: Email collision does not link identities

- GIVEN a BugLab identity and an existing BugLogin account share an email but have no approved provider link
- WHEN federation completes
- THEN the API does not authenticate as that BugLogin account
- AND no session is issued until the approved linking procedure completes

### Requirement: Native desktop federation does not depend on permissive CORS

The desktop MUST perform API discovery and code exchange through its Rust HTTPS transport and use system-browser navigation for the authorization page. The native flow MUST NOT require wildcard CORS or expose provider tokens to WebView JavaScript. Any separate browser-client CORS origin MUST be exact, HTTPS, and compatible with the API's credentialed CORS policy.

#### Scenario: Native desktop exchanges code

- GIVEN a valid callback is received by the desktop
- WHEN Rust exchanges the code with the canonical BugLogin API
- THEN the exchange succeeds or fails based on API authorization without requiring a WebView CORS exception

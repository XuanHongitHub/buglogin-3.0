# Tasks

- [ ] Confirm BugLab ID provider contract: app code, native callback URIs, state support, stable subject, app mapping field, active-status semantics, logout allowlist, and test environment.
- [x] Define the identity-linking and just-in-time provisioning policy; require zero default tenant access.
- [x] Extend the API/OpenAPI contract for SSO start/completion and provider identity mapping.
- [x] Implement a backend-owned BugLab ID exchange client with strict TLS, timeout, response validation, replay protection, redaction, and audit.
- [x] Implement desktop system-browser login and a state-bound callback using the proposed `buglogin://sso/callback` scheme. Provider approval is still required before enabling it.
- [x] Implement safe existing-account linking and role-neutral provisioning, including ambiguous mapping denial.
- [ ] Implement BugLogin session revocation plus approved BugLab ID logout redirect. Local session revocation uses existing logout; IDP redirect awaits its allowlist contract.
- [x] Add focused provider URL/subject unit tests, desktop bridge contract checks, and update OpenAPI/capability/docs. Full API security, role-matrix, and cross-OS acceptance tests remain open.
- [ ] Run a real BugLab ID integration test and record release evidence for 3.0.4.

# Account Recovery Delivery: Unsupported for BugLogin 3.0 Launch

Status: Accepted launch constraint. Outbound account-recovery email and webhook delivery are not supported in this release.

## Decision

BugLogin 3.0 must not depend on an email provider, recovery webhook URL, or recovery webhook secret to start in production. Do not route recovery delivery through BugLogin 1.0 or another project.

## Required behavior

- Production configuration validation must not require `BUGLOGIN_RECOVERY_WEBHOOK_URL` or `BUGLOGIN_RECOVERY_WEBHOOK_SECRET`.
- Recovery request endpoints must return the API's canonical unavailable response (`503 service_unavailable`, or the equivalent established error code) without disclosing whether an account exists.
- An unavailable request must not create, persist, consume, or send a recovery token, and must not mutate account credentials or sessions.
- The web UI must state that account recovery is currently unavailable and give the user a clear next step to contact the BugLogin administrator through an independently documented support channel. Do not present a request as submitted or successful.
- Recovery completion/consume endpoints must remain fail-closed while delivery is unsupported; possession of a caller-supplied token must not bypass the unavailable gate.
- Remove recovery delivery configuration from production runbooks and health checks. Do not log secrets if such variables are present in an inherited environment.

## Scope

This decision disables outbound recovery delivery only. Normal login, session refresh/revocation, and scoped API-token behavior remain separate capabilities and retain their existing authorization and audit requirements.

## Release evidence

Before enabling the production API, verify with targeted API/UI checks that startup succeeds with both recovery variables absent, recovery requests return the unavailable response without state changes, completion cannot consume a token, and the UI does not claim success. This document records the desired behavior; it is not evidence that source code or a deployed release already implements it.

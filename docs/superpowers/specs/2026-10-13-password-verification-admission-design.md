# Password Verification Admission Design

## Problem

Credential authentication performs Argon2id verification for known and unknown identities, but there is no process-local bound on concurrent verification work. A burst of anonymous login attempts can therefore multiply password-hash CPU and memory use.

## Contract

- Limit concurrent Argon2id verifications per API process to four.
- Acquire admission before identity lookup or dummy verification so unknown identifiers are bounded too.
- Return a canonical, retryable `503 credential_capacity_exhausted` immediately when saturated.
- Keep the existing persistent per-account failure count and lock window authoritative; do not add IP identity assumptions or trust unverified proxy headers.
- Release the verification permit immediately after the hash check, before writing the failure/success record.

## Acceptance

- A saturated limiter returns the documented problem without executing a password verification.
- Login/lockout/unknown-identity tests continue passing.
- This is per-process admission only; distributed rate limiting and reverse-proxy request policy remain deployment requirements.

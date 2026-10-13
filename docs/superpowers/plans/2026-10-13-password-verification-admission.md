# Password Verification Admission Plan

Status: implemented and locally verified; deployment-level distributed rate limiting remains outside the process.

- [x] Bound concurrent Argon2id verification to four per process and fail fast with retryable 503.
- [x] Verify saturation behavior plus existing login, lockout, and unknown-identity behavior.
- [x] Record process-local scope and deployment rate-limit requirements.

## Evidence

- `cargo test --locked -p buglogin-api --lib services::credentials::tests` — 4/4 pass, including saturation, unknown identity, password lockout/recovery, and successful credential handling.
- `cargo test --locked -p buglogin-api --test http_auth` — 9/9 pass, covering cookies/CSRF, refresh/logout revocation, device approval, API token scope/revoke, recovery, and owner-scoped durable session management.
- `pnpm generate:api` and `pnpm test:contract` — pass; 24/24 contract tests and canonical registry check. Login OpenAPI now includes the overload `503` response.
- `cargo fmt --all -- --check` — pass.

The limiter is process-local. Production still needs gateway/distributed rate limits, trusted request sizing, and hosted PostgreSQL restart evidence; this implementation does not claim those deployment gates.

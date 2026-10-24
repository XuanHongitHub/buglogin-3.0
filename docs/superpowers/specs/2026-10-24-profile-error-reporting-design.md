# Automatic desktop error reporting

## Goal

Make authenticated profile-launch failures visible to the BugLogin API without requiring the user to open Feedback and attach logs manually. Duplicate reports for the same error code and desktop version must resolve to one durable feedback record.

## Design

- The Tauri `profile_launch` command preserves and returns the original error immediately.
- For an authenticated hosted desktop, it asynchronously submits a redacted diagnostic through the existing authenticated `POST /v1/feedback` contract. Local/unavailable modes and missing sessions remain unchanged.
- The report contains the stable error code/title, HTTP status, correlation ID, app version, OS, and architecture. It excludes profile names/IDs, paths, cookies, credentials, and arbitrary error detail.
- A SHA-256 idempotency key over the stable diagnostic identity makes API persistence idempotent across retries and repeated identical failures.
- Reporting failures are best effort and never mask the original profile-launch error.
- The API's existing feedback persistence is the durable store; this is not an S3 object upload. Server route availability must be checked before release.

## Acceptance

- Tests prove stable dedupe keys, redacted payload shape, and preservation of launch errors when reporting fails.
- Existing authenticated API feedback tests prove durable idempotent storage.
- Production endpoint reachability is checked without sending user or diagnostic data.
- Release documentation states that automatic reports are stored as `support_feedback` records, not raw S3 logs.

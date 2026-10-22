# S3 Endpoint Validation Design

Status: implementation in progress.

## Scope

Validate the configured remote object-store endpoint at the `S3Config` boundary. This is shared by hosted API startup and storage adapters.

## Contract

- Parse the complete endpoint as a URL; require HTTPS and a non-empty host.
- Reject username/password, query, fragment, and control characters. Credentials belong only in the dedicated access-key, secret-key, and session-token settings.
- Preserve a valid optional path prefix and port for S3-compatible providers.
- Never include the configured endpoint string in `Debug`; it may itself contain provider-specific sensitive path data.
- Keep S3 credentials redacted; hold environment-loaded credentials in zeroizing buffers during partial configuration failure, zeroize config copies after client construction, and zeroize any remaining config credentials on drop. Preserve current object naming and TLS certificate verification behavior.

## Acceptance

- Valid HTTPS provider endpoints, including a path prefix or explicit port, remain accepted.
- HTTP, malformed URLs, missing hosts, URL userinfo, query strings, and fragments are rejected before constructing the S3 client.
- Debug output does not contain endpoint text or S3 credentials.
- Regression tests are added; execution is deferred to the single consolidated verification pass requested by the owner.

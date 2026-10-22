# Migration Package File HTTP Boundary

## Decision

`PUT /v1/migrations/packages/{packageId}/files` accepts authenticated `application/octet-stream` uploads only. Read the request body at this route with an explicit 64 MiB limit rather than Axum's default body limit. Use the shared two-slot large-transfer admission pool also used by direct sync import and profile blob upload; keep the separate one-direct-import-per-process lock because sync imports can reach 320 MiB. Reject declared `Content-Length` values above the route limit before taking a transfer slot, then retain bounded body reading for absent/chunked lengths. Return a retryable 503 when the shared pool is full. Reject unsupported media types with 415 and oversized files with the canonical BugLogin 413 Problem before checksum validation or persistence. Requests remain in memory up to their respective ceilings; larger migration data must be split into package files by the client.

## Acceptance

- Valid 3 MiB file upload passes the HTTP boundary (above Axum's default 2 MiB cap).
- Payload above 64 MiB returns `migration_package_file_too_large`/413 without persistence.
- A declared oversized body is rejected before acquiring a large-transfer slot; missing or chunked lengths remain protected by the bounded body read.
- Missing or non-octet-stream content type returns `unsupported_media_type`/415.
- At most two large transfer bodies are admitted across sync import, profile blob upload, and package-file upload per API process; additional requests receive retryable 503. Direct sync imports remain limited to one concurrently.
- Authentication and `migration.write` authorization run before body consumption.
- Both OpenAPI copies document the media type, 64 MiB maximum, and 413/415 responses.
- Regression source is added; execution remains deferred to the consolidated verification pass.

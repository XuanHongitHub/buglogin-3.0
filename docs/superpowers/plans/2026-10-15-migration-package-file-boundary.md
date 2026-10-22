# Migration Package File Boundary Plan

- [x] Replace the default-limited `Bytes` extractor with an explicit bounded request read at the 64 MiB file limit.
- [x] Enforce `application/octet-stream` and return canonical 413/415 Problems before checksum or storage work.
- [x] Bound large transfer body buffering with a shared two-slot pool across direct sync imports, profile blobs, and package files; retain the one-direct-import process lock and document retryable 503 saturation behavior.
- [x] Align both OpenAPI copies and add HTTP regression coverage for a 3 MiB upload.
- [ ] Run package upload integration and contract gates during the consolidated verification pass.

No build, test, or deployment was run for this change.

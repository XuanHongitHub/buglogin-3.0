# S3 Endpoint Validation Plan

- [x] Read the current S3 config, constructor, production startup path, and local test fixtures.
- [x] Parse and validate configured endpoints at construction time.
- [x] Redact the endpoint in `Debug`.
- [x] Add focused regression tests for accepted/rejected endpoint forms and debug redaction; execution remains deferred.
- [x] Hold environment-loaded S3 credentials in zeroizing buffers, erase the configuration copies after S3 client construction, and zeroize on drop, with source regression coverage.
- [ ] Run only the consolidated verification pass after the implementation batch is frozen.

Owner instruction: do not run tests/build during this implementation pass; source assertions are written for the later single verification pass.

# Hosted Sync Import REST Contract Design

## Problem

`POST /v1/migrations/sync/import` is routed and registered as a REST command, but the public OpenAPI contract omits its request body and describes its success response without a schema. The Rust DTO is therefore not discoverable or generatable by clients, and nested import records are not closed against unknown fields.

## Contract

- OpenAPI describes the exact `SyncImportInput`, `SyncExport`, nested manifest, blob, revision, and receipt wire shapes.
- Rust rejects unknown fields throughout the sync-import JSON tree, matching the closed OpenAPI object schemas.
- The bounded reader must consume exactly one complete JSON document (allowing trailing JSON whitespace only) before accepting the request. This also ensures chunked requests cannot evade the 320 MiB limit by placing an otherwise valid document at the start of an oversized body.
- The parser regression gate covers a valid document exactly at the byte cap, rejection of a second JSON document, and rejection of trailing whitespace that exceeds the cap.
- Manifest object keys are confined slash-separated relative paths: no absolute paths, empty/dot/dot-dot segments, backslashes, drive/URI colons, control characters, or keys over 1024 bytes. Names containing `..` within an otherwise valid segment remain valid.
- `dryRun` remains optional with Rust's default `false`; generation, workspace/resource binding, and authorization remain backend authority.
- The endpoint authenticates before reading the body, rejects a declared oversized `Content-Length` before taking a transfer slot, admits one direct import per API process, and shares a two-slot large-transfer body budget with profile blob and migration-package uploads. It accepts at most 320 MiB of direct JSON and returns canonical `413`/`415`/`422` errors. Missing/chunked lengths remain bounded during body reads. Saturated requests receive retryable `503`; larger exports use staged migration-package upload.
- The endpoint remains REST-only. Do not add a second MCP idempotency claim path; any future MCP exposure must call a shared import service.
- JSON blob payloads remain byte arrays for compatibility with the existing Rust endpoint. The 320 MiB cap bounds aggregate input, but large-export memory and throughput still need staging measurements.

## Acceptance

- Runtime and documentation OpenAPI copies match and specify required idempotency, request body, success receipt, and Problem responses.
- Rust request deserialization rejects unknown fields at every nested DTO boundary.
- Regression tests assert schema parity and nested strictness; generated frontend API artifacts are current.
- Existing import authorization, lease, idempotency, crash-recovery, and rollback behavior remains unchanged.

## Release limits

Local contract parity does not prove hosted staging behavior, production sync durability, or suitability of JSON byte-array transport for large exports.

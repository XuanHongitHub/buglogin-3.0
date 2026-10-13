# MCP Input Contract Plan

Status: in progress; MCP schema discovery and boundary validation are implemented, with full adapter fixture parity still open.

1. [x] Inventory registry commands with the MCP adapter and compare each input schema against OpenAPI and Rust deserialization. Found 24 missing definitions and mismatched proxy/event/profile input shapes.
2. [x] Add the missing input schemas to the canonical contract source; close input objects and specify required fields, types, nullability, and bounds.
3. [x] Generate each MCP tool's resolved JSON Schema from OpenAPI; generation fails when an enabled MCP command lacks its schema.
4. [x] Validate MCP input before authentication/authorization/execution without replacing service validation. Validators compile once and use only embedded references.
5. [x] Add a valid fixture for every MCP input schema; all 44 are accepted and `null` is rejected. Boundary tests also cover missing required fields, wrong types, unknown fields, and bounds before executor invocation.
6. [x] MCP execution tests retain ACL/scope, idempotency, audit, replay, and secret-redaction coverage.
7. [x] Finish MCP/OpenAPI/registry and desktop adapter verification, formatting, generated checks, and `git diff --check`.
8. [ ] Re-audit hosted sync command parity separately; do not enable migration sync import through MCP until the shared service and duplicate idempotency claim behavior are resolved.

Production acceptance requires all MCP inputs to have executable schemas and no generic fallback. Until then, MCP tool discovery is not a complete API contract.

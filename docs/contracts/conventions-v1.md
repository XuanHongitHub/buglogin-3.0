# Error, Event, and Idempotency Conventions v1

## Errors

All adapters return one canonical problem object:

```json
{"type":"https://buglogin.local/problems/profile-not-ready","title":"Profile not ready","status":409,"code":"profile_not_ready","detail":"Safe user-facing text","instance":"req_...","correlationId":"cor_...","retryable":false,"fields":{}}
```

- `code`: stable snake_case machine identifier; never parse `detail`.
- HTTP mapping: validation `400`, auth `401`, forbidden `403`, missing `404`, conflict/state `409`, rate limit `429`, internal `500`, unavailable `503`, timeout `504`.
- Tauri returns `Result<T, Problem>`. MCP maps invalid input to `-32602`, unavailable command to `-32601`, auth/permission to `-32001`, conflict to `-32009`, internal to `-32603`; canonical `Problem` remains in structured content.
- Secrets, tokens, credentials, cookies, filesystem roots, and raw upstream responses are redacted.

## Events

Event names use `<domain>.<subject>.<past-tense>`; payloads use camelCase.

```json
{"eventId":"evt_...","eventVersion":"1.0","name":"operation.updated","occurredAt":"2026-07-20T00:00:00Z","correlationId":"cor_...","causationId":"cmd_...","actorId":"usr_...","resource":{"type":"profile","id":"prf_...","revision":4},"data":{}}
```

- Delivery is at-least-once; consumers deduplicate by `eventId`.
- Per-resource ordering uses monotonic `resource.revision`; no global ordering promise.
- Events contain references, status, and redacted diagnostics; never secret material.
- Breaking payload changes require a new `eventVersion`; additive optional fields do not.

## Idempotency

- Required for every mutating or long-running command exposed through REST/MCP; recommended for Tauri retries.
- Key format: opaque ASCII, 16-128 characters. Scope: authenticated actor + workspace + command ID.
- Same key plus same normalized input returns the original result/operation. Same key plus different input returns `409 idempotency_conflict`.
- Claim keys atomically before side effects. Persist terminal result for 24 hours minimum; migration/import keys persist for the rollback window.
- Failed validation/auth does not consume a key. Unknown execution state returns the existing operation ID, never starts a duplicate.

## Deterministic identifiers

- Migration UUIDv5 namespace: `f5d2d691-b715-48b1-bc16-2855ef99586b` (`buglogin.migration.v1`).
- Name input: `source_system:source_instance_id:entity_type:source_id`.
- Canonical IDs are lowercase UUID strings in migration packages; collisions with different provenance stop the import.

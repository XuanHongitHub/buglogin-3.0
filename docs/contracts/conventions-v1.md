# Error, Event, and Idempotency Conventions v1

## Errors

All adapters return one canonical problem object:

```json
{"type":"https://buglogin.local/problems/profile-not-ready","title":"Profile not ready","status":409,"code":"profile_not_ready","detail":"Safe user-facing text","instance":"req_...","correlationId":"cor_...","retryable":false,"fields":{}}
```

- `code`: stable snake_case machine identifier; never parse `detail`.
- REST problems copy the accepted request `x-correlation-id` into `correlationId` and echo it in the response header. Requests missing or supplying an invalid ID receive a generated `cor_...` value in both places.
- HTTP mapping: validation `400`, auth `401`, forbidden `403`, missing `404`, conflict/state `409`, payload too large `413`, rate limit `429`, internal `500`, unavailable `503`, timeout `504`.
- `authorization_state_changed` is a retryable `409`: the caller's authenticated principal or ACL snapshot changed before the mutation was serialized. Dispatch-time authentication rejects before claim; if the serialized MCP recheck rejects after claim, the owned pending claim is released before returning. Retry with fresh authorization state.
- Production MCP revalidates the bearer principal, canonical scope, and current capability after acquiring the control/snapshot mutation gates, so a grant revoked between dispatch authorization and execution cannot authorize a stale mutation.
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
- REST aggregate-control commands derive a fixed-length storage key from actor, workspace, command ID, and client key; the raw key is never concatenated into the bounded control-store key. Replay lookup checks the legacy scoped key first so existing receipts continue to replay across this representation change.
- MCP releases its owned pending claim after authentication/permission denials or the explicit serialized stale-authorization preflight. Other retryable failures may have uncertain side effects and remain pending/replayable for recovery; claims are never released after receipt/audit persistence failures.
- Failed validation/auth does not consume a key. Unknown execution state returns the existing operation ID, never starts a duplicate.

## Deterministic identifiers

- Name input: `source_system:source_instance_id:entity_type:source_id`.

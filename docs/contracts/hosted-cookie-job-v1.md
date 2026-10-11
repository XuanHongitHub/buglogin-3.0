# Hosted Connected-Device Cookie Job Contract v1

Status: design-only; not a registered REST, Tauri, or MCP command.

## Purpose

Define the smallest truthful boundary for hosted cookie mutation. Hosted control-plane services authorize and track work. A connected desktop device remains the only authority that can read or mutate the native profile cookie store.

This document does not add an endpoint, command-registry entry, OpenAPI path, MCP tool, or persistence implementation.

## Current implementation truth

- Device authorization exists through `auth.device.start`, `auth.device.approve`, and `auth.device.token`.
- The desktop stores the resulting bearer in the OS vault, preserves `deviceId`, and uses hosted HTTPS in production.
- `operation.get` is the existing read contract for operation status, but the current operation service is not sufficient evidence of a durable connected-device queue.
- Local Tauri executes `cookie.list`, `cookie.stage`, `cookie.import`, and `cookie.copy` against the local `CookieAuthority`; local REST/MCP expose metadata-only `cookie.list` and reject mutations until device transport exists.
- Hosted production cannot decrypt or directly mutate client-owned native Wayfern cookie databases.
- The current desktop remote adapter intentionally rejects `profile.launch` and `profile.stop` as `production_runtime_sync_not_ready`; no connected-device cookie dispatcher exists.

Therefore, hosted cookie mutation is not production-ready. The contract below is a dependency target, not a claim that the flow currently runs.

## Job envelope

The future control-plane job payload is an internal authenticated message. It is not a public HTTP schema yet.

```json
{
  "schema": "buglogin.connected-cookie-job/1",
  "jobId": "job_opaque",
  "operationId": "operation_opaque",
  "commandId": "cookie.import | cookie.copy",
  "workspaceId": "ws_opaque",
  "deviceId": "device_opaque",
  "profileId": "prf_opaque",
  "expectedGeneration": 7,
  "source": {
    "workspaceId": "ws_opaque",
    "profileId": "prf_opaque",
    "deviceId": "device_opaque"
  },
  "stagedImportRef": "stage_opaque_one_use",
  "requestedBy": "account_opaque",
  "idempotencyKey": "opaque_ascii_key",
  "createdAt": "2026-08-21T00:00:00Z",
  "expiresAt": "2026-08-21T00:10:00Z"
}
```

Rules:

- `cookie.import` requires `stagedImportRef`; `cookie.copy` must omit it.
- `source` is required only for `cookie.copy`; source and destination ACLs are independent.
- `deviceId` identifies the executor, not the account. Account authorization is evaluated by the hosted authority before enqueue.
- `profileId`, workspace IDs, generation, and staged references are opaque identifiers; they never contain cookie data.
- The job must target the device that owns the destination runtime. Hosted services must not accept a client-supplied filesystem root, native profile path, vault key, lease token, or fencing secret.
- `createdAt`/`expiresAt` bound replay. Expired jobs fail closed and do not mutate the profile.
- `idempotencyKey` follows the shared actor + workspace + command scope rules in `docs/contracts/conventions-v1.md`.

The illustrative values above are schema examples only. They are not production fixtures, credentials, or proof of a live endpoint.

## Execution state

The durable hosted record, once an implementation owner exists, must expose only this state machine:

```text
accepted -> dispatched -> running -> completed
                         |       -> failed
                         |       -> cancelled
                         -> expired
```

Additional rules:

- `accepted` means authorization and idempotency claim succeeded; no cookie mutation occurred.
- `dispatched` means a device-targeted message was durably queued. It does not mean the device received it.
- `running` requires a device lease/fencing proof and profile stopped-state proof from the device authority.
- `completed` requires a durable device receipt containing the new generation and redacted counts.
- `failed`, `cancelled`, and `expired` are terminal. Unknown delivery or commit outcome remains non-terminal until reconciliation proves the result.
- A retry reuses the same `operationId` and idempotency receipt. It never creates a second mutation for the same normalized request.

## Device receipt

Only the connected device may produce the mutation receipt:

```json
{
  "schema": "buglogin.connected-cookie-receipt/1",
  "operationId": "operation_opaque",
  "jobId": "job_opaque",
  "deviceId": "device_opaque",
  "profileId": "prf_opaque",
  "previousGeneration": 7,
  "newGeneration": 8,
  "createdCount": 12,
  "replacedCount": 2,
  "deletedCount": 0,
  "metadataChecksum": "sha256_hex",
  "completedAt": "2026-08-21T00:01:00Z"
}
```

Receipt rules:

- No cookie names, values, domains, paths, credentials, vault versions, filesystem paths, lease tokens, or native browser responses.
- `metadataChecksum` covers canonical metadata only; it is not a cookie-value export.
- The device must reject a stale `expectedGeneration`, running/unknown profile state, missing lease, wrong device binding, or expired staged reference before mutation.
- The hosted authority must verify receipt binding to the original job, device, profile, generation, and idempotency key before marking the operation complete.

## Staging boundary

`cookie.stage` is currently a local command that accepts raw cookie values. That shape cannot be forwarded through hosted REST or MCP because the cookie workflow forbids raw values in those adapters.

Until a separate approved device-local staging handshake exists:

- hosted REST/MCP must not expose raw-cookie `cookie.stage`;
- a hosted job may reference only an opaque, one-use stage created on the connected device;
- the stage must expire on device restart, be bound to profile/workspace/device, and never be serialized into the hosted control plane;
- a missing or reused stage returns `staged_cookie_import_not_found` or the existing canonical equivalent, without revealing whether cookie data existed.

## Dependency map

| Layer | Existing authority | Required for hosted cookie jobs | Status |
|---|---|---|---|
| Account/device auth | `device_authorization` + desktop `AuthVault` | Bind job actor to authenticated account and `deviceId` | Existing, needs job integration |
| ACL | control grants/workspace membership | Check source and destination independently | Existing primitives, no job path |
| Operation status | `OperationService` + `operation.get` | Durable hosted queue, restart recovery, unknown-outcome reconciliation | Missing |
| Device transport | desktop `RemoteApiClient` | Authenticated device inbox/outbox, ACK, retry, fencing | Missing |
| Local mutation | browser `CookieAuthority` | Device executor invokes local import/copy with lease and CAS | Existing local path |
| Staging | local `cookie.stage` | Device-bound one-use stage handshake | Missing for hosted |
| Receipt | local `CookieMutationResult` | Signed/bound device receipt persisted by hosted authority | Missing |
| Hosted durability | control persistence / PostgreSQL target | Idempotency, terminal receipt, restart recovery | General primitives exist; cookie job implementation missing |
| Events | operation/event conventions | At-least-once redacted job updates | Generic conventions exist; no cookie event producer |
| Release proof | Windows/macOS acceptance gates | restart, stale CAS, lease loss, offline, native Wayfern verification | Missing |

## Explicit non-contracts

This v1 design does not define:

- a REST path or HTTP status table;
- a new command ID;
- a server-side cookie ledger;
- a hosted cookie-value upload/download;
- direct server access to desktop files, OS vaults, or Wayfern databases;
- automatic fallback to local authority in production;
- fake device acknowledgements, simulated progress, or synthetic production proof.

## Implementation gate

The next implementation packet must be owned by the control-plane/device-transport owners together. It may add service code only after these contracts are approved:

1. durable job/receipt schema and restart recovery;
2. authenticated device inbox with replay, expiry, ACK, and fencing semantics;
3. device-local stage creation/consumption handshake;
4. hosted ACL and idempotency integration;
5. local `CookieAuthority` executor adapter;
6. Windows and macOS evidence for offline, restart, stale generation, lease loss, and native Wayfern verification;
7. registry/OpenAPI/MCP additions only after the service path exists and passes contract-drift checks.

Until then, `cookie.import` and `cookie.copy` remain truthful only for local/Tauri execution, despite their planned adapter metadata.

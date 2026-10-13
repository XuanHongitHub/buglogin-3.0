# Hosted Connected-Device Cookie Job Contract v1

Status: REST control-plane job surface implemented; native connected-device execution remains incomplete. The API endpoints below are not registered as command-registry, Tauri, or MCP commands.

## Purpose

Define the smallest truthful boundary for hosted cookie mutation. Hosted control-plane services authorize and track work. A connected desktop device remains the only authority that can read or mutate the native profile cookie store.

The control plane persists an authenticated job and device-bound state transitions. This is not proof that a desktop device receives the job or mutates a native cookie store.

## Current implementation truth

- Device authorization exists through `auth.device.start`, `auth.device.approve`, and `auth.device.token`.
- The desktop stores the resulting bearer in the OS vault, preserves `deviceId`, and uses hosted HTTPS in production.
- `operation.get` is the existing read contract for operation status, but the current operation service is not sufficient evidence of a durable connected-device queue.
- Local Tauri executes `cookie.list`, `cookie.stage`, `cookie.import`, and `cookie.copy` against the local `CookieAuthority`; local REST/MCP expose metadata-only `cookie.list` and reject mutations until device transport exists.
- Hosted production cannot decrypt or directly mutate client-owned native Wayfern cookie databases.
- The hosted API's `/v1/profiles/{id}/launch` and `/stop` routes intentionally reject server-side runtime execution as `production_runtime_dispatch_required`. Production desktop Tauri commands instead run the browser process on the authorized desktop, using the API for profile/binding/manifest data and lease-fenced sync. This is not web-console-to-device dispatch. Cookie mutations remain fail-closed in the production desktop adapter because no device job consumer or supported native-cookie encryption path is connected.
- The Rust API exposes `POST /v1/cookie-jobs`, requester-scoped `GET /v1/cookie-jobs/{jobId}`, device-bearer `GET /v1/workspaces/{workspaceId}/cookie-jobs/inbox`, and device-bearer claim/ack/complete routes. Enqueue authorizes destination creation and independent source profile read access, scopes idempotency by actor/workspace/command, and derives a ten-minute expiry server-side. Inbox results are filtered by the authenticated device and current executor ACL; claim rechecks those permissions before changing state.
- Desktop `RemoteApiClient` has typed inbox/claim/ack/complete transport methods, validates opaque job IDs before constructing request paths, and sends only the redacted metadata receipt. This is transport plumbing; no background poller consumes those methods yet.
- Enqueue responds `202` with durable `accepted` state. It does not imply device receipt, local execution, or completed sync. No desktop adapter calls these routes yet.
- The public enqueue body contains no actor, idempotency key, creation time, expiry, cookie value, path, or vault data. Unknown fields fail with canonical Problem `422`.

Therefore, hosted cookie mutation is still not production-ready. The implemented API boundary and queue state machine are only part of the full device workflow.

## Job envelope

The persisted control-plane job payload is an internal authenticated record. Its public enqueue request is the stricter `EnqueueCookieJobInput` schema in `crates/api/openapi/openapi-v1.yaml`.

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
| Account/device auth | `device_authorization` + desktop `AuthVault` | Bind enqueue actor to account; bind claim/ack/receipt to device | Enqueue and device-bearer HTTP authorization implemented; desktop client integration missing |
| ACL | control grants/workspace membership | Check source and destination independently | Enqueue checks destination create and source read ACL before persistence |
| Operation status | `ConnectedCookieJobService` + job read route | Durable hosted queue, restart recovery, unknown-outcome reconciliation | Job state and actor-scoped read implemented; dispatcher/recovery and commit reconciliation missing |
| Device transport | desktop `RemoteApiClient` | Authenticated device inbox/outbox, ACK, retry, fencing | Authenticated ACL-filtered inbox and ACK routes exist; desktop poller, retry/restart, outbox, and fencing integration missing |
| Local mutation | browser `CookieAuthority` + `wayfern::materialize_encrypted` | Device executor invokes local import/copy with lease and CAS, then updates the actual stopped browser profile | Not connected: `CookieAuthority` stores secrets in the OS vault; Wayfern accepts only browser-compatible `v10/v11` ciphertext, while plaintext materialization intentionally fails closed |
| Staging | local `cookie.stage` | Device-bound one-use stage handshake | Missing for hosted |
| Receipt | local `CookieMutationResult` | Device-bound receipt persisted by hosted authority | HTTP completion validates job/device/profile/generation binding; native receipt production is not integrated |
| Hosted durability | control persistence / PostgreSQL target | Idempotency, terminal receipt, restart recovery | Queue persistence and replay implemented; hosted PostgreSQL restart evidence and job recovery policy missing |
| Events | operation/event conventions | At-least-once redacted job updates | Generic conventions exist; no cookie event producer |
| Release proof | Windows/macOS acceptance gates | restart, stale CAS, lease loss, offline, native Wayfern verification | Missing |

## Explicit non-contracts

The v1 HTTP contract does not provide:

- a new command ID;
- a server-side cookie ledger;
- a hosted cookie-value upload/download;
- direct server access to desktop files, OS vaults, or Wayfern databases;
- automatic fallback to local authority in production;
- fake device acknowledgements, simulated progress, or synthetic production proof.

## Implementation gate

The next implementation packet must be owned by the control-plane/device-transport owners together. It may add service code only after these contracts are approved:

1. integrate a desktop device poller that securely calls the device-bearer routes and handles retry/restart without duplicate mutation;
2. implement one-use device-local staging and bind it to profile/workspace/device;
3. make the device executor verify stopped state, lease/fencing, generation CAS and expiry before local `CookieAuthority` mutation;
4. reconcile unknown commit outcomes and persist only a receipt verified against the original job;
5. add operation/event visibility without cookie values or secret metadata;
6. prove offline, restart, stale generation, lease loss and native Wayfern behavior on Windows and macOS.

Until then, `cookie.import` and `cookie.copy` remain fail-closed in production REST/MCP and are not wired to the Wayfern profile database. A durable queue receipt must not be reported as a browser mutation. The next implementation gate is a tested, supported Wayfern encryption/materialization path for Windows and macOS, followed by the desktop dispatcher and crash reconciliation.

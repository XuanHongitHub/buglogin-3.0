# Hosted Connected-Device Cookie Job Contract v1

Status: REST control-plane job surface and source-level desktop connected-device execution are implemented; native/runtime and hosted staging verification remain incomplete. The API endpoints below are not registered as command-registry, Tauri, or MCP commands.

## Purpose

Define the smallest truthful boundary for hosted cookie mutation. Hosted control-plane services authorize and track work. A connected desktop device remains the only authority that can read or mutate the native profile cookie store.

The control plane persists an authenticated job and device-bound state transitions. The desktop source includes a device inbox poller and executor, but those source paths are not runtime or hosted-staging evidence.

Desktop polling uses `GET /v1/device/cookie-jobs/inbox` to fetch one bounded batch across the authenticated account's currently authorized workspaces. The API filters by the enrolled device and rechecks destination create and copy-source read permissions before dispatch. An already-running job remains visible only to its recorded executor account and device so an interrupted mutation can be reconciled after ACL changes; each resumed profile read, lease, and manifest write still checks current API grants. Claim repeats authorization before execution. The previous workspace-scoped inbox remains available for compatibility.

## Current implementation truth

- Device authorization exists through `auth.device.start`, `auth.device.approve`, and `auth.device.token`.
- The desktop stores the resulting bearer in the OS vault, preserves `deviceId`, and uses hosted HTTPS in production.
- `operation.get` is the existing read contract for operation status, but the current operation service is not sufficient evidence of a durable connected-device queue.
- Local Tauri executes `cookie.list`, `cookie.stage`, `cookie.import`, and `cookie.copy` against the local `CookieAuthority`. Generic REST/MCP expose metadata-only `cookie.list` and reject raw-value mutations; hosted desktop import/copy uses the separate connected-device job flow.
- Hosted production cannot decrypt or directly mutate client-owned native Wayfern cookie databases.
- CookieAuthority source now persists staged values only in OS-vault entries with metadata-only ledger references and restores unexpired stages after restart. The stage is bound to workspace/profile and expires after 15 minutes; device identity is implicit in the local OS vault.
- The hosted API's `/v1/profiles/{id}/launch` and `/stop` routes intentionally reject server-side runtime execution as `production_runtime_dispatch_required`. Production desktop Tauri commands instead run the browser process on the authorized desktop, using the API for profile/binding/manifest data and lease-fenced sync. This is not web-console-to-device dispatch. Hosted cookie jobs are executed by the connected desktop in source; that implementation has not yet been built or verified against real devices and hosted staging.
- The local desktop adapter now has source-only Wayfern `v10` encryption, a durable authority-side recovery journal, and SQLite replay before local mutation acknowledgment/list/launch. Its synthetic local lease is not a hosted device lease and does not authorize production execution.
- Cookie authority errors now map through one shared `CookieAuthorityError::as_problem` implementation, so local desktop commands and API cookie reads preserve canonical machine-readable codes, HTTP status classes, and correlation IDs. This does not imply hosted cookie execution.
- The Rust API exposes `POST /v1/cookie-jobs`, requester-scoped `GET /v1/cookie-jobs/{jobId}`, device-bearer `GET /v1/workspaces/{workspaceId}/cookie-jobs/inbox`, and device-bearer claim/ack/complete/fail routes, plus requester `cancel`. Job reads require current profile read authority; API-token scopes are enforced through the same capability checks. Enqueue requires an active destination profile binding, authorizes destination creation and independent source profile read access, scopes idempotency by actor/workspace/command, and derives a ten-minute expiry server-side. Inbox results are filtered by the authenticated device and current executor ACL; claim and acknowledgment recheck those permissions before changing state. Failure accepts only a fixed, redacted failure code; it never accepts a free-form diagnostic. Requesters may cancel only before execution enters `running`; later outcomes require reconciliation.
- Desktop `RemoteApiClient` has typed inbox/claim/ack/complete/fail transport methods, validates opaque job IDs before constructing request paths, and sends only the redacted metadata receipt. The first `dispatched -> running` acknowledgment requires the standard owner ID, owner token, and fencing-token headers; the API checks them against the current resource lease using the device bearer before accepting the transition. The lease token is never added to the durable job.
- The authenticated desktop background poller requests one bounded inbox every 30 seconds and retries import and same-device copy jobs. It claims accepted/dispatched jobs, while a running job is resumed only for its persisted executor account and device; the API ACK response must confirm `running` for that executor before mutation proceeds. The executor stages/imports locally, holds the resource lease through profile snapshot sync, and submits a metadata-only terminal receipt. A retry may resume an already-held idle profile only when the path, workspace/resource, owner token, and fencing token still match; validity is checked against the runtime's currently renewed grant, while an active browser or changed/expired held lease remains rejected. Enqueue's `202 accepted` response alone still does not imply device receipt, local execution, or completed sync.
- Regular profile sync recursively includes persistent browser files such as `Default/Cookies` as encrypted profile payload. Cookie jobs are a separate workflow for importing cookie strings or copying cookie data between profiles; they are not required just to preserve a profile's existing login sessions during sync.
- The public enqueue body contains no actor, idempotency key, creation time, expiry, cookie value, path, or vault data. Unknown fields fail with canonical Problem `422`.

Therefore, source wiring for hosted cookie mutation exists, but production readiness is not established until the consolidated build/test pass, device/runtime verification, hosted staging checks, and cancellation/expiry/restart fault cases are complete.

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
  "executorAccountId": "account_opaque",
  "expectedGeneration": 7,
  "source": {
    "workspaceId": "ws_opaque",
    "profileId": "prf_opaque",
    "deviceId": "device_opaque"
  },
  "stagedImportRef": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "requestedBy": "account_opaque",
  "idempotencyKey": "opaque_ascii_key",
  "createdAt": "2026-08-21T00:00:00Z",
  "expiresAt": "2026-08-21T00:10:00Z"
}
```

Rules:

- `cookie.import` requires a 64-character hexadecimal `stagedImportRef` matching the native `StagedImportId`; `cookie.copy` must omit it.
- `source` is required only for `cookie.copy`; source and destination ACLs are independent.
- `deviceId` identifies the executor, not the account. Account authorization is evaluated by the hosted authority before enqueue.
- `executorAccountId` is absent until the authenticated device acknowledges execution, then binds inbox reconciliation and terminal receipts to that same account/device session identity.
- `profileId`, workspace IDs, generation, and staged references are opaque identifiers; they never contain cookie data.
- `expectedGeneration` may be `0` for a profile that has never had a cookie mutation; each committed mutation advances it by exactly one. The maximum `u64` value is rejected because no next generation can be represented.
- The job must target the device that owns the destination runtime. Hosted services must not accept a client-supplied filesystem root, native profile path, vault key, lease token, or fencing secret.
- `createdAt`/`expiresAt` bound the claim/dispatch window. An unclaimed job expires without mutation; after transition to `running`, the device inbox continues to expose it and accepts a terminal receipt/failure after the original deadline so a committed local mutation can be reconciled.
- Revoking profile ACLs prevents new claims and acknowledgments. It does not hide a job already in `running` from its bound executor or prevent that executor from reporting the terminal outcome of work already started.
- Enqueue, claim, first acknowledgment, cancel, and terminal receipt/failure writes carry the control aggregate revision as a transactional read precondition. PostgreSQL locks and checks that row in the same transaction as the job write, so an ACL update on another API instance either commits before the job and makes the check fail, or waits until after the job transition. Same-process handlers also refresh and reauthorize under the shared mutation gate. A stale precondition returns `storage_revision_conflict` (409); retrying the same idempotency key after a failed CAS remains valid.
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
- `running` requires a current resource lease/fencing proof validated by the API and profile stopped-state proof from the device authority. Idempotent acknowledgment of an already-running job remains available for recovery without revalidating a potentially expired lease.
- `completed` requires a durable device receipt containing the new generation and redacted counts.
- `failed` requires the device executor to report one of the schema-defined failure classes while the job is running; repeating the same failure is idempotent, while a different failure after terminal state is rejected.
- `failed`, `cancelled`, and `expired` are terminal. Unknown delivery or commit outcome remains non-terminal until reconciliation proves the result; a running job is never expired solely because its original enqueue deadline elapsed.
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
- the stage must survive application restart until its short expiry so a claimed job can be forward-recovered, be bound to profile/workspace/device, and never serialize cookie values outside the device OS vault;
- the stage expiry is 15 minutes from local creation; the device must recheck it before mutation, and an expired stage causes a no-mutation terminal failure; expired stage secrets are removed during startup recovery;
- a missing or reused stage returns `staged_cookie_import_not_found` or the existing canonical equivalent, without revealing whether cookie data existed.

The durable stage is only preparation, not evidence of a browser mutation. Local execution persists a per-profile forward-recovery journal binding scope, operation, idempotency key, and expected/next generation in the same authority ledger commit; the journal contains metadata only. While it exists, local profile launch and further cookie mutations are fenced. Recovery replays the authoritative vault into the Wayfern SQLite transaction idempotently, then clears the local journal. The hosted executor binds that journal to the original job/device/stage and uses authenticated device authorization and coordination fencing in source; runtime and failure-recovery behavior remain unverified. Missing stage secret, key file, Cookie DB, supported schema, or stopped-profile proof keeps local recovery pending.

## Dependency map

| Layer | Existing authority | Required for hosted cookie jobs | Status |
|---|---|---|---|
| Account/device auth | `device_authorization` + desktop `AuthVault` | Bind enqueue actor to account; bind claim/ack/receipt to device | Enqueue and device-bearer HTTP authorization implemented; desktop integration exists in source, runtime evidence remains open |
| ACL | control grants/workspace membership | Check source and destination independently | Enqueue checks destination create and source read ACL before persistence |
| Operation status | `ConnectedCookieJobService` + job read route | Durable hosted queue, restart recovery, unknown-outcome reconciliation | Job state, bounded cross-workspace device inbox, and source-level desktop import/copy restart executor implemented; runtime and hosted-durability evidence remain open |
| Device transport | desktop `RemoteApiClient` | Authenticated device inbox/outbox, ACK, retry, fencing | Workspace and batched device inbox plus ACK routes recheck current ACL; desktop polling, retry/restart and lease-fenced execution are implemented in source, not yet verified |
| Local mutation | browser `CookieAuthority` + Wayfern `v10` reader/writer | Device executor invokes import/copy with hosted authorization and coordination fencing, then updates the actual stopped browser profile | Source-level executor journals, decrypts source values in memory, materializes SQLite and syncs destination; platform/runtime and restart fault evidence remain open |
| Staging | local `cookie.stage` | Device-bound one-use stage retained in OS vault through its short expiry | Local source path persists and restores stages; hosted job/device binding exists in source. Integrated expiry/cleanup evidence remains open |
| Receipt | local `CookieMutationResult` | Device-bound receipt persisted by hosted authority | HTTP completion validates job/device/profile/generation binding; desktop source produces metadata-only receipt after local commit and profile sync, not yet runtime-verified |
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

Remaining proof work:

1. run the consolidated build/test pass, including duplicate delivery, cancellation/expiry races, and interrupted local journal recovery;
2. verify lease fencing, snapshot sync, and unknown commit reconciliation against the hosted API and PostgreSQL persistence target;
3. verify the native Wayfern database read/write path and restart behavior on supported Windows/macOS builds;
4. confirm offline failures remain recoverable and terminal receipts are sent only after the profile generation commits.

`cookie.import` and `cookie.copy` remain unavailable through generic REST/MCP commands; hosted execution uses the separate authenticated device-job flow. The desktop implementation is source-wired to the Wayfern database and sync path, but is unverified and does not establish production authorization or successful mutation. A durable queue receipt must not be reported as a browser mutation. Remaining gates are the consolidated build/test pass, Windows/macOS runtime verification, hosted staging, and fault/recovery verification.

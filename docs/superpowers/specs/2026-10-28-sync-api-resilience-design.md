# Sync and API resilience on the existing VPS

Date: 2026-10-28
Status: proposed design, grounded in source and read-only production observations. Implementation and production acceptance are incomplete.
Scope: ordinary profile stop/upload/resume/open, legacy blob checks, PostgreSQL control access, and HTTP responsiveness. Companion: `../plans/2026-10-28-sync-api-resilience.md`.

## 1. Outcome

Slow S3 or PostgreSQL must cause bounded, explicit failures in the affected operation while the API continues accepting connections and serving independent requests. A normal sync must upload only changed objects and must not send a separate existence request for every new object. Automatic retry must preserve local data, avoid duplicate work, and converge after dependencies recover.

This is an availability and correctness objective on the current VPS. It does not promise that an unavailable dependency can complete a sync, or that every possible infrastructure 502 can be eliminated.

## 2. Evidence and limits

Read-only production observations on 2026-10-28:

- The API service remained active with no restart/OOM; host RAM and disk were available. The origin listen queue reached 129 against a backlog of 128, and CLOSE-WAIT sockets increased to about 195.
- Origin health/readiness requests timed out while TCP connections could still connect. Cloudflare tunnel failures followed origin unresponsiveness.
- One Tokio worker thread was observed at the stalled process snapshot. This is an observation, not a supported runtime configuration or a reason to fix availability by increasing thread count.
- Slow-request logs included blob checks lasting approximately 11.8 seconds and a manifest PUT lasting approximately 13.9 seconds.
- PostgreSQL worker samples showed service times around 0.86–1.0 seconds with little queue wait. The database activity snapshot later had no active work. Neither observation proves the database caused the entire incident.
- Existing `http_request` logs record slow/error requests, not all requests. Their counts are lower bounds and cannot establish total QPS or which desktop caused traffic.

Source proves that synchronous S3 HEAD/GET/PUT calls occurred directly in async handlers and that member-directory rendering performed one synchronous identity lookup per member. These are defects capable of blocking progress. The exact terminal stalled stack and request origin were not captured; do not describe one dependency as the sole proven incident cause without further evidence.

## 3. Existing mechanisms to preserve

| Concern | Existing authority/mechanism | Reuse |
| --- | --- | --- |
| Write ownership | Redis coordination lease, owner token, fencing token | Validate before writes and revalidate before authoritative commit |
| Cloud truth | Versioned sync manifest, generation and root checksum | Manifest activation remains the publication boundary |
| Changed data | `source_generation` and staged encrypted snapshot cache | Reuse authorized unchanged objects; upload changed staged objects |
| Safe object creation | Server verifies SHA-256; S3 PUT already uses `If-None-Match: *` | Preserve immutable key semantics and duplicate verification |
| Upload bookkeeping | Existing blob idempotency records and reservations | Extend the current owning service if needed; do not add a parallel upload authority |
| Cleanup safety | Object reservations, GC claims, shared/exclusive advisory locks and aggregate CAS | Retain deletion fencing and ownership checks |
| Stop retry | Runtime coordinator retry intent and retained local files | Extend its scheduling and single-flight behavior |
| Ambiguous manifest response | Existing commit reconciliation and local commit marker | Read authoritative manifest before retrying a completed commit |
| Status | Workspace status batch and confirmed lease/runtime projection | Keep runtime status separate from durable sync completion |

Older import/migration tools are not reintroduced. References to existing internal reservation/journal types do not authorize restoring retired public migration APIs.

## 4. Required correction of current WIP

Compile success does not establish that the current patch is deployable.

1. At the first audit, `PostgresControlRecordWorker::get_many` had a batched SQL query but the outer `PostgresControlRecordStore` lacked the forwarding override; trait fallback still called individual `get` methods. A concurrent session added forwarding to the working tree during planning. Verify the real adapter dispatch path before accepting it.
2. At the first audit, member identity loading remained synchronous inside the async route. A concurrent working-tree change now runs the owning read unit in `spawn_blocking`. Verify compilation, error mapping, snapshot semantics and lexical guard lifetimes on the final source.
3. Upload now awaits a spawned S3 task between advisory-lock acquisition and explicit release. Handler cancellation can skip release while the blocking task continues. The task must own acquisition, validation, object write and guaranteed cleanup as one lifecycle; a handler timeout is not cleanup.
4. Async upload currently copies a `Bytes` body into a `Vec`. Avoid the additional full payload copy, and retain both memory accounting and admission ownership while a detached blocking task continues.
5. The eight-slot S3 limit is a preliminary value, not a measured capacity guarantee. It needs timeout, memory, fairness, and cancellation evidence.
6. Readiness still calls synchronous PostgreSQL/S3 work from an async method. Monitoring must not become another event-loop blocking path.

Source reconciliation: main advanced from `4b0d3444` to `dbf7b356` during this planning session, while the two corrections above were still visible as additional uncommitted changes. Planning did not change application source or verify the concurrent implementation. Recheck HEAD/diff before execution; do not reapply work already integrated.

## 5. Target request flow

```text
browser process stopped
    -> one sync coordinator for this profile
    -> read current authorized manifest / acquire current write authority
    -> stage a consistent encrypted snapshot
    -> reuse eligible objects from current manifest
    -> PUT changed objects, bounded by desktop-global admission
         -> API validates identity, resource, checksum, lease and limits
         -> bounded blocking work owns reservation/GC lock + payload + permits
         -> conditional object creation, then verified receipt
    -> commit candidate manifest using revision/fencing checks
    -> reconcile ambiguous response against authoritative manifest
    -> publish synced/stopped receipt, clear completed local markers
    -> release lease using existing runtime rules

dependency error
    -> retain dirty data and the attempt identity
    -> one scheduled retry with backoff and jitter
    -> fresh lease/fence and base-manifest reconciliation before writes
```

## 6. Remove normal-path blob preflight

`upload_profile_snapshot` currently calls `blob_present` for each changed object, retries that check, then sends PUT. Unchanged manifest objects are already excluded using `source_generation`.

The normal new-client path will send changed-object PUTs directly. It retains the same immutable staged ciphertext, generation, object checksum and idempotency key for transport retries of one attempt. A changed base manifest, key version, binding or fence triggers explicit reconciliation; it is not silently treated as the same upload attempt.

Removing the client preflight does not remove server verification. The existing server S3 `store_put` still performs HEAD before conditional PUT, and manifest commit currently downloads objects for validation. Both must be counted when measuring improvement.

For a successful attempt with C changed objects, the normal client removes C HTTP GET checks: approximately `2C + 1` blob/commit calls become `C + 1`, excluding leases, auth, metadata and retries. This is not a claim about total application requests or all S3 operations.

The old GET check route remains functional for installed clients, with identical permission and fence checks and bounded dependency work. Do not return a fabricated `present` value or retire the route during the rollout.

## 7. Conditional PUT and replay semantics

Optimize S3 storage only after checking the actual configured provider with the installed rust-s3 version.

- Candidate default: attempt conditional PUT first for a new physical key, eliminating the preceding HEAD on successful new writes.
- Success means the verified bytes were accepted under the intended immutable key.
- On 412, verify stored checksum and size before acknowledging a duplicate. Do not use ETag as SHA-256.
- On 409, distinguish a conditional-write race from a content conflict; reconcile existence/checksum before a bounded retry.
- Timeout or lost response is ambiguous. Reconcile the same key/checksum; never overwrite or advance the manifest on an assumed result.
- A reused idempotency key with different input remains a conflict.
- Current upload replay downloads the full object. Replace unnecessary replay GET with verified metadata/receipt reconciliation only where the existing integrity contract permits it.
- Do not cache a missing/present result across generation, tenant/resource, key-version, GC or authorization changes.

Amazon S3 behavior is a reference, not proof that the deployed S3-compatible provider implements it correctly.

## 8. Resume and the batch-check decision

Direct idempotent PUT is the initial recovery path. A new batch endpoint is not required to remove normal-path checks.

Measure interrupted uploads after that change. If resending acknowledged payloads materially dominates recovery time/bandwidth, add one bounded metadata-only resume query to the existing blob capability. Proposed limits: 128 entries and 1 MiB request body; exact schema, command/MCP mapping and provider evidence must be settled before implementation.

Each result must distinguish verified present, missing, conflict, and verification unavailable. An unavailable check is never `missing` or `present`. Requests require the same current workspace/resource/lease authority as the existing staged-object check. IDs, checksums and sizes are boundary-validated and duplicate/conflicting entries rejected.

Use durable completed-upload receipts where valid; a reservation alone proves intent, not object existence. A batch HTTP wrapper that still runs unbounded individual S3 HEADs is not an optimization. Fallback S3 checks remain bounded.

A local checkpoint may accelerate recovery of the exact staged snapshot, but cannot authorize skipping an object after GC/reservation expiry or lost server state. Revalidate remote evidence or retry the immutable PUT. Reuse existing projection/cache/commit-marker persistence first; any additional checkpoint requires atomic writes, schema version, checksum verification and safe crash recovery. Do not introduce deterministic AEAD nonces to make retries appear stable.

## 9. Keep HTTP progress independent of bulk I/O

- Offload every synchronous S3 and PostgreSQL call reachable from async HTTP handlers, including authentication/policy lookups, readiness, upload replay, reservations, advisory-lock operations and manifest validation. Audit filesystem, checksum and encryption work on large payload paths too.
- Use existing Tokio blocking facilities with admission acquired before scheduling work. Bound task count, retained bytes and wait time; moving unbounded work to the blocking pool is insufficient.
- Transfer ownership of permits, payload and cleanup to the actual work. Keep accounting until work and response-body ownership end, including client disconnect.
- Add S3 transport deadlines in the installed blocking client. An outer `tokio::time::timeout` cannot stop an already running blocking operation.
- Isolate small metadata operations from bulk transfer monopolization. Eight total remote slots and four transfer slots are starting candidates; choose the final split from current-host measurements and leave headroom under the existing memory ceiling.
- Avoid retaining process-wide control snapshot or mutation guards over S3/network waits. Capture an authorized snapshot, perform external work without a global guard, then refresh and reauthorize at the authoritative mutation boundary. Preserve revision/fencing and denial semantics.
- Short overload responses include `Retry-After` and a stable retryable code. Protect lease renewal, status and liveness from bulk-transfer queues; their underlying services can still report a real dependency failure.
- Liveness checks local runtime health. Readiness checks dependencies using bounded, coalesced work and truthfully reports failure. Neither uses an unbounded or synchronous external call on Tokio workers.

## 10. PostgreSQL and locking boundaries

The current `ControlPersistence` mutex and single synchronous PostgreSQL worker serialize operations. Merely creating more PostgreSQL workers behind the same outer mutex does not produce useful concurrency.

First remove N+1 queries, offload synchronous calls and add operation-name/queue/service timing. Investigate slow operations with redacted query classes and PostgreSQL wait/lock evidence. Configure connection, statement and lock timeouts through owned source/deployment configuration; apply per-operation rules to avoid killing legitimate commits arbitrarily.

Measure write amplification too: per-object reservation and receipt updates may repeatedly serialize/persist the whole control aggregate. Record statements, serialized bytes and aggregate CAS conflicts per sync, not only time per individual SQL call. Reuse `reserve_sync_blob_uploads` for a bounded authenticated preparation batch if reservation writes dominate; define its transport/authority contract before exposing it. Moving reservation/receipt records out of the aggregate is not allowed unless atomic quota, publication and GC ownership guarantees are preserved.

Do not add `recv_timeout` to mutations and assume a timeout cancelled the operation. The worker may commit after the caller stops waiting. Preserve command identity and reconcile the durable receipt before another mutation attempt.

Keep transactions atomic and locks on the session that acquired them. Cancellation, panic, connection loss and unlock failure require explicit behavior. A connection whose advisory lock ownership is uncertain must not return to general use. Changing to a bounded connection pool is a separate implementation packet only if measured latency still fails after the above work; all same-session/advisory/CAS semantics must pass first.

Retain the current ACL freshness contract. Do not reduce DB reads by extending an authorization cache TTL without committed invalidation and fail-closed freshness semantics.

## 11. Retry and single-flight contract

Existing transport GET retry, per-object retry and whole-sync retry must share one retry budget. Avoid their multiplicative combination.

- One active attempt per profile; triggers during it coalesce into at most one follow-up dirty generation. Different profiles remain independently schedulable.
- Retain the two-per-profile upload limit initially and add a bounded desktop-global limit so opening many profiles does not multiply concurrency without bound.
- Suggested transient retry schedule: full jitter up to 1, 2, 4, 8, 16 and 30 seconds, honoring server `Retry-After` and an elapsed-time/attempt budget. Final values are configurable within validated bounds.
- Retry connect failures, eligible timeouts, 429 and explicit retryable 5xx. A 401 allows the existing single auth refresh path. A 403, corrupt payload, checksum conflict or revoked grant does not enter a background retry loop. A fence/revision conflict invokes reconciliation, not blind transport retry.
- Stop retries when offline, auth is gone, profile is deleted or the app is draining. Reconnect wakes one attempt with jitter. Pause passive UI polling when hidden; do not pause a necessary upload solely because the app loses focus.
- Respect the existing bounded lease hold and dirty-data preservation. Retry after releasing a lease must reacquire current authority and compare the base manifest. No arbitrary force-unlock of another active device.

## 12. Status and data safety

`running` is backed by process/runtime evidence. `syncing` means an actual active attempt or verified queued retry. `stopped` describes process state; synced cloud data additionally requires the authoritative manifest receipt. The UI may retain the last usable status with freshness/error detail but must not claim synchronized after a failed read or upload.

A dependency outage preserves local files and automatic recovery intent. Concurrent changes from another device require deterministic conflict handling; this work does not authorize last-write-wins data loss, authorization bypass, synthetic successful sync, or opening an empty profile over unavailable encrypted data.

## 13. Observability

Record all-route counters/status/duration histograms, active requests, event-loop delay, remote operation kind/time/outcome, permit utilization/rejections, live retained-byte budget, PostgreSQL operation name/queue wait/service time and retry attempt/budget. Log one attempt summary with object counts, reused/changed bytes, upload count, client precheck count, dependency operations, commit time and final outcome.

Use route templates and fixed operation names as metric labels. Correlation IDs belong in logs, not high-cardinality metric labels. Never log bearer/owner tokens, credentials, raw object paths, cookies, encryption keys or SQL parameters. Per-device attribution, if needed, uses an existing approved opaque identifier or scoped hash and retention policy.

## 14. Failure modes and acceptance

| Failure | Required behavior |
| --- | --- |
| S3 stalls or all metadata calls slow | Finite transport deadline; bounded active work; liveness/status continue making progress |
| Request cancelled during upload | Work retains permits/bytes/lock lifecycle; cleanup occurs; later retry reconciles any completed object |
| PUT accepted, response lost | Same immutable attempt reconciles; no overwrite or duplicate manifest generation |
| Manifest committed, response lost | Existing reconciliation identifies the published root; dirty state is not incorrectly re-uploaded as a conflicting generation |
| GC races upload/commit | Existing reservation, advisory and revision fences prevent deletion of referenced data |
| PostgreSQL times out after commit uncertainty | Durable idempotency reconciliation; no blind duplicate mutation |
| Old desktop keeps checking every blob | Compatibility route remains bounded and responsive; metrics identify its cost |
| Many profiles retry simultaneously | Desktop-global budget and jitter prevent synchronized bursts |
| Lease lost/revoked role/key rotated | Stop writes, preserve local data, reacquire/reconcile only with valid authority |
| Huge body/slow reader/disconnect | Byte budget stays accurate until memory is actually released |

Acceptance targets and reproducible validation are specified in the implementation plan. Production success requires measured runtime evidence, not compile-only checks.

## 15. Exclusions and references

No VPS upgrade, new broker, custom Redis blob queue, browser adapter rewrite, manual recovery-key flow, blanket removal of access/lease/GC checks, speculative cache authority, destructive DB cleanup or new updater/landing redesign is part of this design.

Primary references consulted 2026-10-28:

- [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): blocking tasks that have started cannot be aborted; admission and transport deadlines remain necessary.
- [Amazon S3 conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html): conditional object creation and 409/412 outcomes.
- [PostgreSQL advisory locks](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS): session lock lifetime and cleanup obligations.
- [PostgreSQL statement and lock timeouts](https://www.postgresql.org/docs/current/runtime-config-client.html): timeout scope and transaction implications.
- Installed `rust-s3` 0.37.2 `Bucket::set_request_timeout`: source documents a 30-second default and backend-specific support; validate the active backend rather than assuming cancellation.

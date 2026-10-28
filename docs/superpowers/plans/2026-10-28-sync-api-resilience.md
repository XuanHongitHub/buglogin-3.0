# Sync/API resilience implementation and rollout plan

Date: 2026-10-28
Status: implementation in progress; local API and desktop packets are on main, but cancellation, provider deadlines, measured capacity, load evidence and deployment acceptance remain incomplete.
Design authority: [Sync and API resilience design](../specs/2026-10-28-sync-api-resilience-design.md).
Related existing work: [API and profile capacity](2026-10-27-api-and-profile-capacity.md), authoritative profile status spec, existing GC fencing and release contracts.

## 1. Success and boundaries

Deliver normal profile syncing on the existing VPS without per-file HTTP existence preflight, with bounded dependency work and automatic recovery that does not cause another request storm. A delayed S3 operation must not prevent independent HTTP requests from being accepted. Preserve user data, permissions, object integrity, revision CAS and lease ownership.

Use existing services and standard Tokio/PostgreSQL/S3 facilities. Work directly on the shared main branch; inspect ownership and concurrent diffs before each packet. Keep each implementation packet small and independently reviewable. New services, pool redesign and batch transport are conditional decisions with explicit evidence gates, not prerequisites to the first API fix.

This plan does not authorize running a load attack on production, deleting cloud/local profile data, changing another session's work or making direct production SSH edits. Production activation follows the documented committed-source/artifact/deploy path. Availability recovery may include one controlled service restart after evidence capture; restart is containment, not acceptance of the code fix.

## 2. Source findings and current WIP status

| Finding | Source | Required disposition |
| --- | --- | --- |
| Every changed object gets `blob_present` before upload | `apps/desktop/src-tauri/src/desktop_state.rs::upload_profile_snapshot` | Remove normal-path check after API duplicate-write behavior is proven |
| GET transport retry and outer presence retry both wait 250 ms | `remote_api.rs::send`, `desktop_state.rs` | Consolidate retry ownership and budgets |
| Two uploads are bounded per snapshot, not necessarily per desktop | `MAX_CONCURRENT_PROFILE_BLOB_UPLOADS` | Add desktop-wide admission without serializing unrelated profile lifecycle commands |
| S3 adapter HEADs before conditional PUT | `crates/storage/src/remote/s3.rs::store_put` | Measure; condition-first optimization requires provider verification |
| Manifest commit reads object bytes | `control.rs::commit_resource_manifest` | Offload safely; retain integrity checks until replacement proof exists |
| GET middleware holds snapshot read guard through handler | `control.rs::refresh_hosted_snapshot` | Audit slow external I/O under the guard; shorten safely with final reauthorization |
| Async readiness calls synchronous dependencies | `http/mod.rs::ApiState::readiness` | Offload/coalesce with bounded dependency deadlines |
| Worker batch SQL initially unreachable through default outer fallback; forwarding now visible in concurrent WIP | `control/mod.rs::PostgresControlRecordStore` | Verify actual dispatch and final implementation |
| Member identities initially loaded synchronously in async route; offload now visible in concurrent WIP | `control.rs::member_directory_views` | Verify the owning read unit, error mapping and guard lifetimes |
| Async upload cancellation and advisory unlock | `http/mod.rs::put_reserved_remote_chunk_async`, `control.rs::put_resource_blob` | Keep work/lock/cleanup ownership together; cancellation regression is mandatory |
| Current async wrapper copies request body | `http/mod.rs::put_remote_chunk_async`, upload route | Move/shared-own bytes and retain memory permits until actual completion |
| Current 8-slot limiter was shared by metadata and bulk | `http/mod.rs::remote_chunk_io_slot` | Split metadata and transfer admission; keep per-resource fairness and measured tuning as open work |
| S3 blocking client had no explicit request deadline | `crates/storage/src/remote/s3.rs`, installed `rust-s3 0.37.2` backend | Apply separate metadata/transfer request timeouts; account for SDK retries when setting operation budgets |

The current compile and format checks are useful source evidence. They do not prove one-query dispatch, cancellation safety, production responsiveness or correct deployment. The previous statement that member reads were already one PostgreSQL query was premature at the first audit. Main advanced from `4b0d3444` to `dbf7b356` while planning; subsequent forwarding/offload corrections are visible in shared WIP and must be verified rather than implemented twice. This planning turn made no application-source changes.

## 3. Dependency order and deliverables

```text
P0 baseline + correct WIP claims
    |
    v
P1 cancellation-safe bounded API I/O ---- P2 real PostgreSQL batch/offload
    |                                          |
    +-------------------+----------------------+
                        v
              G1 API resilience evidence
                        |
                        v
              API artifact -> deploy -> canary
                        |
                        v
P3 direct PUT + single retry coordinator + desktop-wide budget
                        |
                        v
              G2 interrupted sync / A-B evidence
                        |
                        v
              desktop release -> canary
                        |
              +---------+----------+
              v                    v
P4 conditional-first S3      P5 batch resume / DB pool
provider gate               only if measured need persists
```

P1/P2 are logically independent but shared paths have one writer; implement sequentially on main when scopes overlap. Every packet includes code review, focused tests and documentation of residual risk. Do not let optional P4/P5 delay the API safety release.

## 4. P0 — Capture a reliable baseline and audit the pending patch

Owning scope: IO-AUTO / IO-DATA diagnostics, existing API HTTP/storage files, capacity plan. No deployment mutation during baseline capture.

- [ ] Record source commit, current diff ownership, API executable hash, deployment receipt/commit, start time, runtime thread configuration and active unit limits. Do not infer API version from desktop UI version.
- [ ] Match current logs to that artifact; classify route counts as slow-log counts until complete counters exist.
- [ ] Capture a bounded incident sample: listen backlog/socket states, process threads/waits, PostgreSQL wait events/locks/slow operation classes, S3 operation timing, memory/cgroup throttling and tunnel errors. Avoid secrets and full process environments.
- [ ] Compare origin and public responses with one bounded probe each. Distinguish a tunnel/proxy 502 from an application-generated 502/problem response; retain correlation and version evidence rather than grouping every failure under one cause.
- [ ] Capture a redacted object/byte distribution from representative existing local profiles and server attempt summaries: changed/reused/chunked object counts, maximum object, total changed bytes and retained staging size. Inspect existing ephemeral-path exclusions without deleting profile data.
- [ ] Inventory every synchronous persistence/object-store/filesystem call reachable from HTTP handlers and readiness, including nested auth/policy helpers. For each record: handler, sync operation, guard held, expected duration, existing offload and admission.
- [ ] Review lock ordering: snapshot gate -> mutation guard -> persistence mutex -> advisory connection/object stripe; identify cycles and locks held over network waits.
- [ ] Correct the WIP gaps in Section 2 before marking the patch release-ready. Confirm no unrelated README/image/untracked-file changes are included in the patch.
- [ ] Add low-cardinality counters and attempt summaries to existing logging/metrics plumbing; name PostgreSQL operation classes without SQL parameters. Use complete route counts to locate caller amplification.

Exit evidence: a route/dependency/guard inventory and artifact identity attached to the existing plan/evidence system, with facts separated from hypotheses. No claim of a sole terminal deadlock cause without a captured stack or reproducible regression.

## 5. P1 — API stays responsive during object-store failures

Primary files: `crates/api/src/http/mod.rs`, `http/control.rs`, `crates/storage/src/remote/s3.rs`, focused API tests and existing deployment configuration when needed.

- [ ] Keep synchronous S3 operations outside Tokio executor threads for blob check/get/put, replay validation and manifest object verification. Extend the existing adapter boundary rather than adding a second store.
- [ ] Make upload work one cancellation-safe ownership unit: admission + retained body + reservation validation + advisory-lock lifecycle + conditional write + unlock. Acquire/operate/release on the owning blocking execution path with an explicit cleanup guard. A disconnected HTTP handler must not own the only unlock path.
- [ ] Define cleanup on success, storage error, panic, join failure, process loss and uncertain unlock. Quarantine/reset an uncertain dedicated DB session; never return it to general work as if unlocked. Preserve GC/shared-lock semantics.
- [ ] Preserve late-result reconciliation: a timed-out request may still have written a blob. Same key/checksum/idempotency rechecks outcome safely. Do not start duplicate blocking tasks for a timed-out attempt before their existing work is bounded/reconciled.
- [ ] Move `Bytes` or shared immutable payload ownership into the worker, avoiding an extra full `Vec` copy. Attach retained-byte permits to actual payload lifetime, including detached tasks and response streams.
- [ ] Separate request-count limits from byte limits. Budget uploads, downloads and commit validation together; choose limits with measured base RSS and headroom under the current service memory ceiling. Four 64 MiB bodies plus duplicate buffers can exceed available process headroom.
- [x] Validate S3 transport timeout support for the installed blocking backend. Establish metadata and transfer request timeouts at 5 and 30 seconds. The installed `rust-s3 0.37.2` blocking backend maps this to attohttpc's maximum request duration after TCP connection; SDK retries and backoff can extend the whole logical operation beyond that single-request deadline. Production/provider tuning remains open.
- [ ] Bound bulk and metadata concurrency before spawning work. Evaluate a total candidate of 8 remote operations with reserved metadata capacity; use per-resource admission/fairness so one object stripe or profile does not occupy every permit. Keep transfer-body admission compatible with the remote limit.
- [ ] Return overload promptly with canonical retryable 503 and `Retry-After`; 429 is reserved for an actual policy rate limit. Never wait behind an unbounded semaphore/worker queue.
- [ ] Shorten control snapshot guards around external I/O. Preserve early auth and perform fresh authorization/revision/fence validation before publishing a manifest or recording an authoritative mutation. No stale-ACL shortcut.
- [ ] Offload/coalesce readiness dependency work and bound probe concurrency. Liveness must not call PostgreSQL/S3. Failed readiness remains a real failure; cache freshness and probe scheduling must not fabricate recovery.
- [ ] Keep the old `/blobs/check` route functional so existing production desktops benefit from the API-only deployment.

Exit G1-A: a single-worker Tokio integration harness with deliberately blocked S3 still serves liveness and unrelated in-memory requests; cancelled upload eventually releases its advisory lock and all real memory/operation permits; body counters remain accurate until work finishes.

## 6. P2 — Fix PostgreSQL work at its real boundary

Primary files: `crates/storage/src/control/mod.rs`, `crates/api/src/services/control_persistence.rs`, `identity_persistence.rs`, `http/control.rs`, relevant API/storage tests.

- [ ] Implement `PostgresControlRecordStore::get_many` forwarding as one worker operation to the existing SQL `ANY(record_ids)` query. Empty batches return without DB work; dedupe IDs and preserve input validation.
- [ ] Prove wrapper dispatch with an instrumented adapter, and prove one actual PostgreSQL statement for a multi-member page in isolated integration evidence. Memory-store-only tests cannot establish this property.
- [ ] Keep identity visibility filtered by server policy; missing identity remains optional, corrupt records remain errors, and a viewer cannot obtain hidden identity fields through batch loading.
- [ ] Offload the entire synchronous member-directory read operation. Place RwLock guard scopes inside the synchronous function so no guard survives into await. Preserve the original ACL snapshot/invalidation rules.
- [ ] Audit other HTTP sync calls: principal/session reads, control mutation callbacks, reservation reads/writes, advisory acquire/release, runtime inbox and readiness. Move owning critical sections off Tokio while retaining transaction and revision boundaries.
- [x] Move the per-request account lookup in bearer authentication out of Tokio executor workers using bounded blocking admission; overload fails fast with retryable 503 plus `Retry-After`, and the desktop honors that delay with capped deterministic jitter. Runtime responsiveness and overload regression evidence remain required before G1.
- [ ] Instrument worker dispatch name, queue depth, outer persistence mutex wait, DB service time and transaction size. Distinguish query latency, JSON serialization, lock waiting and CPU throttling before choosing an optimization.
- [ ] Measure full-aggregate write amplification for per-object reservations and blob receipts: SQL statement count, persisted JSON bytes and CAS conflicts per sync. If it dominates, use the existing `reserve_sync_blob_uploads` primitive through a bounded authorized preparation contract; do not split quota/reservation authority into independent non-atomic rows as a shortcut.
- [ ] Add appropriate connection/statement/lock deadlines through owned source configuration. Test transaction rollback and session reuse after timeout.
- [ ] Reject queued work when its bounded admission is exhausted. Do not time out a mutation reply and pretend it was cancelled: reconcile its durable receipt if completion is uncertain.
- [ ] Do not add a connection pool behind the existing serial mutex and call it concurrency. P5 may redesign that boundary if measured latency still fails after batching/offload.

Exit G1-B: member page has one batch identity statement, no sync DB wait on Tokio, fresh permission checks still deny revoked access, and dependency delay produces a bounded operation error while liveness stays responsive.

## 7. G1 — API evidence and first production delivery

- [ ] Run targeted API/storage behavior and cancellation tests, Rust formatting and API build. Broaden to shared auth/ACL/manifest/lease/GC integration tests because those boundaries are touched.
- [ ] Run the controlled matrix in Section 12 on a staging/local service with equivalent quota/memory limits and real PostgreSQL plus the configured class of S3 provider. Never generate a stress burst against the jammed production API.
- [ ] Review source and documentation claims against measured results. Fix any admission/cleanup failure before building the artifact.
- [ ] Commit only the reviewed packet on main, push to `XuanHongitHub/buglogin-3.0`, and build the native Linux API through `.github/workflows/native-api-artifact.yml`.
- [ ] Follow `scripts/release/deploy-api-systemd.ps1`: synchronized clean main, attested manifest and hashes, DryRun, predecessor/rollback evidence, activation, public verification and canary. Do not bypass dirty-worktree/provenance checks or discard another session's work to satisfy them; coordinate source completion.
- [ ] Keep deployed API/source/version evidence independent of desktop release tags. Preserve old routes and payloads during this rollout.
- [ ] Observe canary for at least 30 minutes including real old-client sync; inspect backlog trend, dependency latency, memory, errors and lock cleanup. Continue passive monitoring through normal usage after the active canary.

Exit G1: record exact API deployment receipt and observed results. API deployment success alone does not claim the new desktop request strategy has shipped.

## 8. P3 — Desktop normal sync has no per-object GET preflight

Primary files: `desktop_state.rs`, `remote_api.rs`, `profile_projection.rs`, `profile_runtime.rs`, existing `sync_features.rs` scheduler if it owns the trigger, focused desktop tests.

- [ ] Remove `blob_present` from the ordinary changed-object upload loop. Retain `source_generation` skips and descriptor/blob checksum validation. Send direct PUT with existing lease/fence/idempotency headers.
- [ ] Preserve the exact staged ciphertext and attempt identity during transport retry. Audit encrypted snapshot cache reuse and behavior across restart; do not reuse an idempotency key after changing payload, generation or resource/key version.
- [ ] Move blocking snapshot reads/checksum/encryption work off desktop async workers when large; use the existing staging ownership and verify bytes at the boundary.
- [ ] Keep only one active sync attempt per profile. Repeated stop/process-exit/online/manual triggers join or schedule one follow-up generation. Verify the existing runtime queue before adding a map/task owner.
- [ ] Enforce desktop-wide upload admission in addition to the existing two-per-snapshot cap. Start from a measured small limit, initially four total blob uploads, and keep different profile runtime commands responsive.
- [ ] Include cold-open downloads in the same overall memory/network accounting while preserving a small independent metadata/lease budget. Record download object count and bytes; an upload-only improvement is not evidence that opening a large profile is fast.
- [ ] Assign retry ownership explicitly: transport returns enough classification/Retry-After metadata; the sync coordinator owns object/attempt backoff. Preserve generic transport retry for other callers where needed but opt out when the coordinator already owns the budget.
- [ ] Use capped jittered backoff, a finite attempt/elapsed budget and existing durable retry intent. No nested immediate 250 ms retries. Respect offline/auth/draining state and resume one attempt after recovery.
- [ ] Reconcile a changed manifest or lost fence with the current authority before retrying. Keep local dirty files on all uncertain outcomes; never force overwrite another device's generation.
- [ ] Reuse commit reconciliation and marker logic for lost commit responses. Confirm post-commit key enrollment/status failure does not cause already committed data to be treated as an uncommitted new generation.
- [ ] Preserve lifecycle status semantics: only a manifest receipt establishes cloud sync success. Transient polling failure retains last usable state and freshness; do not oscillate falsely between synced/dirty.
- [ ] Count changed/reused objects, actual precheck requests, payload bytes and total retry attempts for each attempt summary. The new normal path must report zero `/blobs/check` calls.

Exit G2: A stops/syncs; B opens the committed generation with matching cookies/storage/extension data; loss of a PUT or commit response recovers automatically; simultaneous triggers do not duplicate work; running locks and revoked access remain correct.

## 9. P4 — Reduce S3 operations, with provider evidence

- [ ] Count client HTTP calls separately from backend S3 HEAD/GET/PUT and manifest validation reads. Removing client GET preflight does not remove the adapter's HEAD or commit-time object GETs.
- [ ] Verify conditional-create semantics of the actual S3-compatible provider: new key, existing matching key, mismatched checksum, concurrent writers, delete race, lost success response and 409/412 classification.
- [ ] If supported, remove unconditional pre-PUT HEAD for new writes and retain checksum/size verification for duplicates and ambiguous outcomes. Keep the existing adapter interface and key layout.
- [ ] Replace full-object download on upload replay with valid metadata/receipt reconciliation where integrity semantics allow. Do not use ETag as the content checksum.
- [ ] Evaluate commit-time re-reading of unchanged objects separately. Skip only with a proof based on validated predecessor manifest, immutable key, unchanged encryption key version, and complete GC fencing. New object receipts must be durable and exact before replacing any new-object verification.
- [ ] If proof/provider evidence fails, retain the verification read with bounded I/O; explicitly report remaining operation cost instead of weakening validation.

Exit: measured reduction of S3 operations/bytes with the same corruption/conflict/GC protections. P4 is not required for the first availability fix.

## 10. P5 — Conditional follow-ups with decision gates

### A. Resume batch

Implement only if interrupted-transfer evidence after P3 shows repeated acknowledged payload bytes materially harm recovery.

- Reuse current blob capability; publish explicit schema/permission/API/MCP/audit semantics and generated contracts through the normal process.
- One resource/attempt per request, current lease/fence required, at most 128 entries and 1 MiB metadata request; reject conflicting duplicates.
- Return per-object verified present/missing/conflict/unavailable. Use valid completed-upload receipts first; reservation or local checkpoint alone is not presence proof.
- Bound any fallback HEAD checks with the API remote admission budget. Measure the saved HTTP round trips and remaining S3 operations separately.
- If the evidence identifies reservation write amplification, a separately specified fenced preparation batch may reuse `reserve_sync_blob_uploads`; keep proposed object metadata, quota reservation, completion receipts and existence evidence distinct. Batch resume must never interpret a prepared reservation as a completed upload.
- New desktop negotiates support from an authoritative capability or version contract. An old API can use direct idempotent PUT fallback; do not add recursive 404 retry or silent permission fallback.
- Persist exact snapshot resume state only if the existing encrypted cache/commit marker is insufficient. Atomic metadata, path confinement, no secrets, no unverified skip after GC/expiry.

### B. PostgreSQL concurrency

Implement only if measured DB service/queue latency still misses G1/G2 targets after P2.

- Remove the single serial outer mutex as a concurrency bottleneck before expecting a pool to help.
- Use a small bounded established pool; separate read work from serialized authoritative mutation units as the existing transaction model allows.
- Pin each transaction/advisory lock to its acquiring session; uncertain timeout/connection state is quarantined. Preserve CAS, idempotency, ACL freshness and resource fences across instances.
- Do not increase global PostgreSQL max connections or add Redis as an ACL/status substitute to mask the cause.

Each follow-up gets a measured go/no-go record. If deferred, state the residual bandwidth/latency cost and keep correctness intact.

## 11. Focused verification map

```text
HTTP handler -> admission -> blocking work -> provider/DB -> receipt -> release
     |              |             |             |           |          |
     |              |             |             |           |          + cancel/panic/unlock tests
     |              |             |             |           + lost-response/replay tests
     |              |             |             + conditional PUT, SQL timeout/CAS tests
     |              |             + no Tokio block / finite provider deadline
     |              + byte bound / count bound / fairness / fast reject
     + auth/lease denial / legacy compatibility / liveness under dependency stall

desktop triggers -> single attempt -> changed blobs -> direct PUT -> manifest
         |              |                 |               |           |
         |              |                 |               |           + ambiguous commit reconcile
         |              |                 |               + same-attempt retries / 412 reconcile
         |              |                 + reused objects excluded / zero GET preflight
         |              + one shared retry budget / global concurrency
         + repeated stop/online/events coalesce / crash retains dirty data
```

Required targeted tests:

| Scenario | Assertion |
| --- | --- |
| Single Tokio worker, fake S3 blocked on a controlled signal | Parallel liveness and unrelated routes complete before S3 is released |
| Full S3 slot budget | Additional work rejected promptly; no new blocking task spawned |
| Same object/stripe contention | Bound active work and prove other allowed metadata/control requests make progress |
| Disconnect at each upload stage | No leaked advisory lock, body budget, transfer slot or orphaned accounting |
| 64 MiB payload and slow reader | No extra full copy; byte ownership remains counted through real completion |
| PostgreSQL `get_many` through real wrapper | One dispatch and one batched SQL statement; zero work for empty batch |
| Member visibility / revoked grant | Batch lookup does not reveal identities or serve stale access |
| S3 accepted PUT then socket dropped | Retry reconciles same key/checksum; no overwrite |
| Commit response lost | Exactly one published generation; reconciliation recognizes root |
| DB timeout with uncertain commit | Durable receipt determines result; no blind duplicate mutation |
| GC versus upload/commit | Neither cancellation nor retries break existing fencing |
| Old check client plus new direct-PUT client | Both work against the API-first deployment |
| N simultaneous profile triggers | One attempt per profile and bounded total uploads/retries |
| Offline -> online / restart | Dirty data retained, one jittered retry, current lease reacquired |
| Another device publishes / key rotates / ACL revokes | Reconciliation or intentional denial, never forced successful sync |

Reuse existing test harnesses and controllable store adapters; avoid wall-clock sleeps where barriers/fake clocks can establish order. Real provider tests remain necessary for conditional-write semantics.

Suggested commands after implementation, scoped first:

```powershell
cargo fmt --all -- --check
cargo check --locked -p buglogin-api --all-targets
cargo test --locked -p buglogin-api --test http_control
cargo test --locked -p buglogin-api --lib <focused_resilience_filter>
cargo test --locked -p buglogin-storage <focused_batch_or_s3_filter>
cargo check --locked -p buglogin-desktop-app --lib
cargo test --locked -p buglogin-desktop-app <focused_upload_or_retry_filter>
pnpm --dir apps/desktop typecheck
git diff --check
```

Angle-bracket filters denote tests to add/name during implementation, not commands already executed. Use the single approved build target and avoid deleting a target whose binaries/processes are active. Broaden shared auth/lease/manifest/GC tests after focused tests pass; do not repeat the full suite without a new reason.

## 12. Controlled performance and failure matrix

Exercise a staging/local API with the current VPS resource limits and capture actual object counts and bytes. Start with two desktops; test larger client counts only as an explicit capacity experiment.

| Workload | Dataset / injected condition | Required evidence |
| --- | --- | --- |
| Metadata-only stop/sync | No changed payload, normal dependencies | Zero blob preflight/upload; target completion within 2 seconds excluding browser shutdown |
| Small normal change | 1, 10 and 100 changed objects; about 1 MiB total | Zero normal-path check GETs; small-sync target within 3 seconds on measured healthy dependencies |
| Mixed snapshot | 1,000 objects with 10 changed | Only changed objects uploaded; count remaining commit S3 reads honestly |
| Large transfer | One maximum-sized object, then a representative large profile | Bounded memory and progress; report throughput rather than promise small-sync latency |
| Slow S3 | HEAD/GET/PUT delayed 5 seconds and transport-timeout path | Liveness p99 target under 250 ms; no 1-second liveness timeout; no sustained accept backlog growth |
| S3 unavailable | Connection failure / 5xx / ambiguous response | Bounded retry and retained dirty state; independent routes progress; readiness truthfully degraded |
| Slow PostgreSQL | Read wait, lock wait, uncertain mutation result | Bounded error/reconciliation; liveness responsive; no stale ACL success |
| Reconnect storm | Two desktops, several profiles and repeated focus/online events | Global admission respected, triggers coalesced, retry QPS and jitter recorded |
| Concurrent access | A running; B starts; then A stops/syncs and B opens | No false unlock; B sees committed data once authority transfers |

Initial status target when Redis/PostgreSQL are healthy: batch status p95 below 750 ms during bulk S3 delay. Readiness must fail/complete within its explicit probe budget (initial target 2 seconds), not hang. These are acceptance targets to measure, not existing achieved numbers.

No 20-team/50-client capacity claim follows from a two-desktop pass. Record p50/p95/p99, request/error totals, S3 operation counts, DB query count, queued work, retained bytes, CPU throttle and memory peak. Time-to-sync is evaluated against changed bytes and dependency timings, not an arbitrary universal deadline.

Gates: no OOM, unexplained restart, lock leak, unsafe stale authorization, missing committed objects or data loss; liveness continues under dependency delay; backlog/CLOSE-WAIT counts return toward baseline after injected work ends; successful recovery produces one authoritative manifest.

## 13. Desktop delivery and rollback

- [ ] After G2, commit/push the desktop packet on main and run the repo's signed Windows/macOS release workflow. Record both OS evidence before declaring a desktop release accepted.
- [ ] Publish stable only from the verified artifact; let the existing approved stable/landing mechanism use that artifact/version. This plan does not modify updater trust/signing rules.
- [ ] Canary the new desktop with two actual accounts/devices, then compare new-client zero-precheck metrics with retained old-client traffic.
- [ ] API predecessor remains deployable and supports the unchanged direct-PUT contract. Keep additive schemas/routes backward compatible. If optional batch transport ships, the client has direct-PUT capability fallback.
- [ ] Roll back on unhealthy liveness, sustained backlog, memory-budget failure, lock leaks, checksum/ACL regressions or an unsafe status transition. Preserve local staged files and durable receipts during rollback.
- [ ] If a true database schema change becomes necessary, apply the project's snapshot/dry-run/rehearsal/verification/rollback requirements before activation. The initial packets require no destructive schema change.

## 14. Review decisions, risks and completion ledger

Architecture decisions: retain existing authority and libraries; API-first compatible rollout; direct idempotent PUT before new resume transport; bounded blocking work with real cleanup ownership; no speculative pool or authorization-cache shortcut.

Code-quality decisions: fix missing dispatch at the storage boundary; consolidate retry ownership; keep lock scopes explicit and cancellation-safe; avoid adding another job/status service.

Test decisions: failure-ordering and single-worker regressions are required because format/build checks passed while real logical gaps remained. Provider/DB evidence supplements controllable adapters; it does not replace them.

Performance decisions: distinguish HTTP request savings, S3 request savings, SQL counts, retained bytes and end-to-end latency. Eight remote slots are a candidate, not a final capacity fact. Conditional PUT and batch resume are measured improvements, not permission to skip integrity checks.

### Implementation evidence — 2026-10-28

- Added a process-wide large-transfer budget that independently caps active body/response streams at four and admitted payload memory at 128 MiB. Admission is acquired before upload body collection, download allocation, idempotency replay reads, and each manifest validation read; upload ownership moves into the blocking worker and download ownership remains with the response body until it is consumed or dropped. Saturation returns a retryable 503 immediately instead of waiting ten seconds in an in-memory queue.
- Added `large_transfer_budget_bounds_retained_bytes_and_releases_admission`; it proves 128 MiB exhaustion, permit recovery after drop, and the independent request-count cap. `cargo test --locked -p buglogin-api large_transfer_budget_bounds_retained_bytes_and_releases_admission --lib`, `cargo test --locked -p buglogin-api --test http_leases --test http_blob_limits` (5/5), `cargo check --locked -p buglogin-api --lib`, `cargo fmt --all -- --check`, and `git diff --check` pass.
- The S3 adapter now configures separate 5-second metadata and 30-second transfer request deadlines using the installed blocking backend. Source inspection confirms attohttpc applies the maximum duration after TCP connection; rust-s3 retries and backoff can extend a logical operation beyond one request timeout. A storage regression test checks both configured values.
- Remote I/O admission now caps total S3 operations at eight and bulk transfers at four, preserving four slots for metadata when bulk is saturated. Its unit test proves this reservation and fail-fast recovery. Per-resource fairness remains open.
- Bearer authentication now owns its bounded admission and `block_in_place` at the shared authorization boundary, so REST, MCP transport, coordination authorization, and nested HTTP reauthorization cannot bypass it. Overload remains retryable `503` with `Retry-After: 2`; API unit tests (284/284) and MCP transport tests (4/4) pass.
- Profile recovery now clears stale active-session/sync-commit markers only when the old local projection still matches its recorded file set and checksums. Regression coverage proves unchanged local data proceeds to remote hydration while changed local data remains preserved and blocked as a real conflict; profile projection tests pass (24/24). This is not conflict merging or permission to overwrite either device's data.
- Profile runtime heartbeats now dispatch lease renewals as independent bounded tasks and return results to the actor, so a slow lease API cannot serialize Stop/launch/status commands. Regression tests hold renewal and remote release requests open and prove local Stop/completion finish within 250 ms; runtime coverage also passes the retry, lease-loss, hold-expiry, and restart-recovery cases. Stop now closes the local process before cloud binding/sync work, signals proxy-bridge shutdown immediately, and waits for bridge cleanup in the background so a bridge task cannot hold the runtime actor. When a stopped local snapshot has a verified active-session marker tied to the current manifest, launch reuses its held lease and opens the preserved local tree without uploading first; the manifest is checked again while that lease is held, and remote movement returns a conflict. At app startup/runtime reconciliation, valid durable session/commit markers are rediscovered after restart; the recorded process lock is checked and unknown/running processes are never synced. Housekeeping sync now runs directly under the per-profile lock instead of creating a new user operation record each poll. Retryable dependency failures use backoff; permanent access denials and manifest conflicts are suppressed until explicit user retry while local data stays on disk. The desktop Stop sync now has a single 180-second total deadline, including its missing-lease retry; timeout is retryable and queues local reconciliation while preserving the stopped snapshot. Production-adapter end-to-end evidence, measured deadline tuning, and signed Windows/macOS release evidence remain open.
- The 128 MiB figure is an explicit initial bound, not a measured cgroup-safe capacity. It accounts for admitted payload bytes only, not allocator overhead, DB aggregates, manifest JSON, TLS buffers, or concurrent non-transfer work. Compare it to the deployed service memory ceiling and measure peak RSS in the controlled matrix before G1. Deterministic disconnect/stalled-S3 cleanup, provider/production deadline tuning, full sync-handler persistence audit, and one-worker responsiveness tests remain open.
- Advisory connection cleanup follow-up — 2026-10-28: blob PUT body/admission and `SyncObjectWriteLockGuard` already move into one `spawn_blocking` owner, so dropping the HTTP handler does not abort the active blocking write. The PostgreSQL adapter still retained its dedicated advisory-lock client when unlock returned false or errored. It now removes and drops that uncertain connection for both shared and exclusive locks; a focused map-lifecycle regression verifies only the uncertain shared owner is discarded. `cargo fmt -p buglogin-storage -- --check` and `cargo test --locked -p buglogin-storage dropping_uncertain_shared_lock_client_preserves_other_lock_owners --lib` pass. A real PostgreSQL cancellation/disconnect test and proof of server-side lock release under network partition remain open.

| Packet | Current status | Done requires |
| --- | --- | --- |
| P0 baseline/inventory | Partial observations available | Artifact match + complete request/dependency inventory |
| P1 API I/O safety | WIP, release blockers identified | Cancellation, deadline, byte-budget, responsiveness evidence |
| P2 PostgreSQL batching/offload | Concurrent WIP now contains forwarding/offload corrections | Actual adapter batch and offload proof on final source |
| G1 API deploy | Not done | Attested deployed artifact + healthy production canary |
| P3 desktop request/retry changes | Not done | Direct-PUT/single-flight/retry tests + A/B convergence |
| G2 desktop release | Not done | Signed OS evidence, published artifact, canary |
| P4 S3 operation reduction | Proposed | Actual provider proof + operation/byte measurements |
| P5 resume batch / DB pool | Conditional | Documented measured need and separate reviewed contract |

This is a source-informed engineering plan, with architecture/code-quality/test/performance review notes. It is not a claim that an independent cross-model review, formal interactive review gate, load test or production acceptance has already passed. No implementation or deployment approval is implied for unrelated changes.

# Sync Object GC Fencing Plan

## Phase 1 — durable contract

- [x] Record the race and required cross-instance invariant in the design spec.
- [x] Add backward-compatible object-key to cleanup-journal claims to the hosted control aggregate.
- [x] Atomically claim GC candidates with aggregate CAS and prevent REST/sync-import manifest publication while claimed.
- [x] Delete only claimed, journal-created objects; atomically release this journal's claims and retire its cleanup journal.
- [x] Expire stale `Staging` journals into cleanup and serialize sync-import object writes with the same workspace/resource advisory lock used by GC. A staging write rechecks its durable journal revision/state and active GC claims while holding that lock; GC also preserves objects referenced by another active staging/finalizing import.
- [x] Allow the original idempotent sync-import request to reopen its journal after cleanup reaches `Abandoned`, using a fresh expected revision and clearing only prior attempt bookkeeping.
- [x] Reload stale `Finalizing` journals before recovery and CAS them to cleanup even when another commit has reached/passed their target generation; add regression source for superseded generation and cleanup. Test execution remains deferred to the consolidated pass.
- [x] Share a journal-scoped advisory lock between sync import and stale `Staging`/`Finalizing` recovery; recovery reloads the durable journal under the lock, so a renewed resource lease during a long transfer cannot be mistaken for a dead writer. Regression source is written; execution remains deferred.

## Phase 2 — evidence

- [x] Add an in-memory multi-state CAS test proving a GC claim rejects a stale manifest writer; the existing referenced-object recovery test covers the manifest-already-active case.
- [x] Add an opt-in PostgreSQL session-lock contention test across independent store handles.
- [x] Prevent PostgreSQL session-lock re-entry through the single-worker connection from granting a false second exclusive lock; add the same-handle case to the opt-in contention test. Code and regression source updated; execution remains deferred.
- [ ] Run both winner-order and lock tests against the live staging PostgreSQL endpoint.
- [x] Verify restart retry, active-reference preservation, and delete failure; claims remain owner-scoped and resumed objects are not cleanup candidates.
- [x] Protect live staged-blob reservations from cleanup claims; expired reservations are durably converted into cleanup-pending journals. Sync import persists its complete reserved object set and atomically reserves quota before PUT, then releases reservations after a durable cleanup transition. Consolidated verification remains deferred.
- [x] Include `reserved_objects` from every active `Staging`/`Finalizing` journal in reservation release and GC claim fences; add a regression test source for an active reserved object with no surviving reservation row. Test execution is deferred to the requested consolidated verification pass.
- [x] Reject conflicting duplicate storage keys within a quota-reservation batch before changing durable state; add regression coverage for checksum/size disagreement. Verification remains deferred.
- [x] Scope committed-object checks used for quota, upload validation, and reservation release to the owning workspace/resource; keep global reference checks only for physical-object GC. Add cross-workspace regression source; execution remains deferred.
- [x] Stream canonical JSON request hashing directly into SHA-256, avoiding a second serialized buffer for large byte-array sync imports.
- [x] Parse the bounded HTTP sync-import body through a streaming reader so the serialized JSON body and decoded blob payloads are not resident in memory together. Keep the `MAX_SYNC_IMPORT_REQUEST_BYTES` cap for both Content-Length and chunked bodies; source written, consolidated compile/tests deferred.
- [x] Move validated import blob buffers into the staging plan instead of cloning them; manifest-only revalidation at finalization retains lease, authorization, and generation checks without retaining a duplicate payload.
- [x] Compute blob-set checksums from sorted references, avoiding a deep clone of blob data when only key/size/checksum metadata is hashed.
- [ ] Verify peak memory and hash compatibility with representative maximum-size exports in the consolidated verification pass.
- [ ] Verify the stale-staging/write/GC winner orders, retry after a stale staging journal is fenced, and shared-lock behavior with independent API instances.
- [x] Run focused API sync recovery, both CAS order guards, shared advisory lock, full API test targets, contract suite, and formatting checks.
- [x] Rehearsal run waived by owner. Keep live PostgreSQL/S3 interrupted-transfer and ambiguous-outcome scenarios in the single consolidated verification pass after implementation freeze; do not create repeated staging rehearsal cycles.

## Release boundary

This source change does not constitute live PostgreSQL/S3 evidence or staging release approval. Live adapter scenarios remain deferred to the owner's single consolidated verification pass; the waived rehearsal is not represented as completed evidence.

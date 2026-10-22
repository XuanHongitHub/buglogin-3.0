# Sync Object GC Fencing

## Status

Implemented in source. The owner waived rehearsal runs and requested one consolidated verification pass after implementation freeze; live PostgreSQL/S3 multi-process evidence is therefore still absent. This code closes the cross-instance race in its designed transaction path, but source review is not live adapter proof.

## Contract

- PostgreSQL control aggregate is the authority for both active manifests and pending object-GC claims.
- Claiming an object for deletion and finalizing a manifest share the same aggregate revision CAS. Exactly one can win a race.
- A manifest commit must reject while any pending GC claim covers one of its physical object keys.
- A GC claim is keyed by physical storage key and journal ID. Multiple abandoned journals can claim one object; no claimant may release another journal's fence.
- A `Staging` or `Finalizing` import journal protects every key in its reserved, created, and resumed object sets. This journal fence remains authoritative if a reservation row is missing or was released during an earlier retry.
- The import writer holds a PostgreSQL journal-scoped advisory lock through completion. Recovery must acquire the same lock and reload the journal before expiring stale `Staging` or `Finalizing` work; age alone is not proof that the writer is gone because the resource lease may be heartbeating during a long object transfer.
- One reservation batch cannot assign different checksums or sizes to the same physical storage key. Identical duplicates are idempotently deduplicated before quota is calculated.
- A manifest only satisfies quota, upload-validation, and reservation-release checks for its owning workspace/resource. Physical GC alone checks references globally because object deletion must preserve any remaining reference.
- Large direct imports must not create redundant full-payload copies: JSON request hashing streams into SHA-256, and the already-validated blob buffer is moved into staging. Finalization revalidates export metadata and current manifest state without requiring the payload buffer.
- The bounded HTTP import parser must stream the JSON request body into the deserializer instead of buffering the full serialized body alongside the decoded blob arrays. Keep the existing request-size cap and reject over-limit chunked bodies before accepting the import.
- Blob-set hashing sorts references and hashes metadata only; it must never deep-clone blob payload bytes.
- GC may delete only an unreferenced object recorded as created by its cleanup journal. Resumed objects are never deleted.
- The claim remains durable until checksum-conditional deletion succeeds and the cleanup journal becomes terminal in the same control transaction.
- Restart retries outstanding claims. Unknown database outcomes fail closed and leave the object/fence intact.
- A stale `Finalizing` journal must be reloaded after refreshing the aggregate before recovery changes it. If the journal remains `Finalizing` past the stale threshold, recovery CAS-transitions it to cleanup even when the current manifest has reached or passed its target generation; atomic finalize updates the manifest and journal together, and cleanup rechecks active manifest/journal references before deleting any object.

## Concurrency cases

1. Manifest transaction commits first: GC's control revision CAS fails; refresh finds the active reference and preserves the object.
2. GC claim commits first: manifest transaction's control revision CAS loses or observes the claim and rejects; deletion proceeds safely.
3. Two cleanup journals claim one key: per-journal claim ownership prevents the first cleanup from unfencing the second.
4. Process loss after claim or object delete: recovery sees durable claim and repeats checksum-safe cleanup before atomically clearing the claim and retiring the journal.

## Acceptance

- Cross-instance control-store CAS test for commit versus GC claim.
- Manifest activation rejects a claimed physical key.
- Multiple cleanup claims keep the key fenced until all journals finish.
- Delete failure or ambiguous claim completion preserves the fence and object/journal for retry.
- Existing sync recovery and migration tests remain green.

## Local implementation evidence

- Hosted control aggregate persists per-physical-object GC claims with backward-compatible deserialization.
- PostgreSQL session advisory locks serialize recovery workers across store handles. Because PostgreSQL locks are reentrant within one session and this adapter multiplexes requests over a single connection, the adapter also tracks session-held lock IDs and rejects a second acquisition by the same worker; unsupported stores fail closed. The shared memory store models the same exclusive lock behavior.
- Sync import and stale-journal recovery share a journal-scoped PostgreSQL advisory lock. Recovery defers when a writer owns it, refreshes and reloads the durable journal after acquiring it, then applies the stale cutoff to the current revision. Regression source verifies a held writer lock prevents a stale staging journal from being fenced; execution remains deferred.
- REST manifest commit and hosted sync-import finalization reject manifests that reference a claimed object.
- Recovery atomically claims unreferenced journal-created objects, checksum-deletes them, then removes only its own claims while retiring the journal in one control-store transaction.
- Local evidence: full API unit/integration targets; both CAS winner-order guards across API state instances; delete-failure claim persistence and restart retry; exact-object fencing; shared memory advisory-lock contention; contract suite.
- An opt-in PostgreSQL session-lock contention test covers same-handle re-entry and competing store handles. Live adapter environment variables were absent, so the configured test target returned without contacting PostgreSQL or S3.
- Not yet proven: real multi-process PostgreSQL CAS ordering, live S3 ambiguous outcomes, and full transfer interruption. These are listed for the single consolidated verification pass, not repeated rehearsals.

# ADR-002: File-First Persistence and Isolated Deployment

- Status: Accepted for v1
- Date: 2026-07-20

## Decision

Keep upstream-compatible file persistence initially. Metadata uses versioned JSON; browser data remains in isolated profile directories; large binary data is separate from metadata. Introduce a database only after measured profile-count or atomicity limits, behind a repository boundary and explicit migration.

Every write uses temp file, flush/fsync where supported, atomic rename, and schema version. Unknown fields survive safe forward reads. Runtime process state is reconstructed, never durable truth. Secrets use OS-protected storage or strict file permissions.

Deploy 2.0 against a new data root, database/schema, object-store prefix, and encryption namespace. Keep 1.x source data and deployment recoverable through rollback. No live dual-write in the first migration.

## Consequences

- Low migration risk from upstream formats.
- Import stays copy-only, dry-run first, idempotent, and rollback-capable.
- Database adoption requires evidence, repository versioning, and a migration plan.
- Storage and deployment isolation prevents accidental legacy mutation.

## Rejected

- Ad hoc JSON/database dual writes.
- In-place migration of the only production copy.
- Durable `running` or `process_id` as lifecycle truth.

## Desktop deployment modes

- `local` is the standalone/self-hosted product. Rust may use the local versioned repository and OS vault as authority. Loopback development may use SQLite for isolated tests or PostgreSQL with mandatory TLS when matching hosted persistence behavior; PostgreSQL credentials and certificates remain outside the repository.
- `production` is the hosted product. Workspace, member, resource, profile metadata, proxy metadata, permissions, audit, jobs, and sync state use the hosted HTTPS API as authority.
- Production desktop must not start a local control server, open a local metadata repository, or fall back to local/self-hosted behavior when the API is unavailable.
- Production metadata, permissions, proxy configuration, profile manifests, runtime projections, audit, and payload mutations cross the hosted API. Local disk holds only runtime binaries, transient staging, and replaceable encrypted profile projections.
- Production may keep browser runtime binaries and encrypted profile payload cache required to launch a local browser process. Cache is replaceable projection data, not authority, and reconciles through the API/sync protocol.
- `BUGLOGIN_DEPLOYMENT_MODE` is mandatory. Production rejects `BUGLOGIN_DATA_ROOT`; the hosted desktop is pinned to the exact `https://api3.buglogin.com` origin with no URL path, query, port override, or redirects. The 1.0 `api.buglogin.com` host is not accepted as a 3.0 API authority.
- Production initializes a Rust-only hosted API client and never opens the local metadata repository. Health, profile list/create, and operation reads cross authenticated HTTPS. Commands whose hosted sync/lease contract is incomplete fail closed with explicit `production_*_not_ready` errors; they never fall back to local authority.

## Hosted sync authority

- PostgreSQL durable control records are the only production authority for the current sync manifest and migration activation state. Hosted reads must derive from that committed state.
- Redis owns leases, fencing counters, presence, and rebuildable events/cache only. Redis manifest payloads are not authoritative and must not participate in hosted migration activation.
- Object storage holds immutable content-addressed encrypted blobs. Uploaded blobs remain invisible until one PostgreSQL transaction promotes the manifest and completes the durable idempotency receipt.
- Hosted migration writes a durable `staging` journal before object-store side effects. Finalization atomically checks the destination base, writes the target aggregate, marks the journal `active`, records audit, and completes idempotency.
- An unknown PostgreSQL commit result is reconciled by probing the durable journal/idempotency record through a fresh connection. Blob deletion is forbidden until the commit outcome is proven absent.
- Memory state is a projection of committed PostgreSQL state. A node that cannot refresh after commit must fail health/drain traffic; it must not restore an older in-memory manifest over committed state.

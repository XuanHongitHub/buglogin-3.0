# MCP Idempotency Claim Release Design

## Problem

Production MCP claims an idempotency key in dispatch before the executor's serialized authorization recheck. If that recheck rejects a stale principal or revoked grant, retaining the pending claim makes a safe retry with the same key appear permanently in progress.

## Contract

- A claim may be released only by its owning claim token and only while it is pending.
- Release atomically deletes the matching pending receipt; a completed receipt, a different token, or a different request checksum cannot be released.
- The production dispatcher releases claims only after authentication/authorization denials or the explicit stale-authorization result from the serialized preflight. Other retryable failures may have uncertain side effects and remain durable/pending for recovery.
- Both regular and async package-activation dispatch follow the same rule.
- PostgreSQL validates ownership while holding a row lock and checks that exactly one row was deleted.

## Storage adapter conformance

- `ControlRecordStore` implementations must explicitly implement readiness, replay lookup, owned-claim abandonment, and advisory lock acquire/release. Defaults that report ready, miss a replay, or return a runtime `501` are not valid production adapter behavior.
- Remote chunk adapters must explicitly implement readiness and checksum-conditional deletion. A runtime `501` during recovery/garbage collection is too late to discover that an adapter cannot honor the storage contract.
- In-memory and PostgreSQL/S3 adapters implement the same required surface. Wrappers forward capabilities explicitly; test adapters must state their behavior instead of inheriting production-shaped defaults.
- `McpExecutionPolicy` implementations must explicitly settle and abandon idempotency claims. A policy that inherits a no-op `Ok(())` default can report a successful MCP mutation without storing a replay receipt.

## Acceptance

- Storage coverage proves an owner can release a pending claim and a new attempt can claim that key.
- PostgreSQL and memory implementations preserve checksum/token fencing.
- MCP dispatch paths release only on the defined failure classes.
- Contract docs distinguish pre-claim dispatcher auth from the serialized executor recheck.
- Formatting, targeted tests, and the consolidated API verification pass before release.

## Transport snapshot locking

- Read tools acquire the shared control snapshot read gate through authorization and data execution so reads use one control revision. Release it before the dispatcher persists the resulting audit event; audit persistence itself uses the mutation boundary.
- Mutation tools must not acquire that write gate in the HTTP transport and then dispatch into `ProductionMcpExecutor`; the executor owns the non-blocking write gate and mutation lock. Re-acquiring the same non-reentrant gate inside the executor rejects every mutation as `control_snapshot_busy`.
- Transport dispatch does not refresh the snapshot before a mutation. The executor refreshes it after acquiring its mutation boundary.

## Mutation completion and audit durability

- For synchronous production MCP mutations, the snapshot gate and control mutation lock cover execution, audit append, and idempotency completion. The receipt and mutation therefore cannot commit from a control aggregate interleaved with another REST/MCP mutation.
- The audit event is appended before the aggregate is serialized into the idempotency completion transaction. If completion fails, the aggregate is reloaded from durable state.
- Standalone MCP audit events (preflight denials and replays) persist their updated control aggregate under the same gates. When an authorization-stale claim is abandoned, the retryable claim is deleted first and its audit event is persisted as a separate control command.
- Read results without an idempotency claim persist their audit event as a standalone control command after the read snapshot is released. Query receipts, when supplied, are then completed without rewriting the control aggregate.
- If standalone audit persistence fails, restore the in-memory aggregate from durable state and surface the persistence error where the caller reports audit failures.

## Scope

Do not auto-release claims after uncertain side effects or output/audit persistence failures. Those remain pending for explicit recovery so the dispatcher cannot accidentally duplicate a completed mutation.

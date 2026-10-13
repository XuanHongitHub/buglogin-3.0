# Hosted Profile Archive/Restore Design

## Scope

Enable `profile.archive` and `profile.restore` for hosted production authority through REST and MCP. Local authority behavior stays unchanged. Permanent hosted deletion remains blocked.

## Authority

- Rust control persistence is the only hosted mutation authority.
- REST and MCP use the same profile revision, binding state, ACL, audit, and durable idempotency semantics.
- React/Tauri may request mutations but cannot mutate hosted metadata locally.

## Mutation

- Require authenticated `profile.archive` or `profile.restore` permission on the profile scope.
- Require a valid 16-128 byte ASCII idempotency key and explicit expected profile revision.
- Require the canonical managed profile binding.
- Archive rejects a runtime-projected `running` profile with `409 profile_running`.
- Archive atomically sets `Profile.archived=true` and `ProfileResource.archiveState=archived`.
- Restore atomically sets `Profile.archived=false` and `ProfileResource.archiveState=active`.
- A successful mutation increments only the profile revision; runtime projection remains unchanged.
- Same key and normalized input replays the original result. Changed input returns `409 idempotency_conflict`.
- Persistence failure restores the in-memory aggregate. Ambiguous completion is recovered from durable replay/reload.

## Evidence

- Hosted REST archive/restore, stale revision, replay, changed-input conflict, running denial, and store reopen tests.
- Hosted MCP archive/restore uses hosted aggregate and survives store reopen.
- API, MCP, format, contract, and release gates pass.

## Deferred

- Hosted permanent delete needs quarantine plus object-retention cleanup proof. REST and MCP share one typed fail-closed response until that capability is implemented; neither transport may fall through to the disabled local profile repository.
- Proxy credentials need a workspace encryption-envelope authority before hosted persistence.

## Local profile deletion contract

- REST and MCP call the same Rust profile-deletion service.
- Require an authorized profile scope, explicit `QUARANTINE_PROFILE_DATA` confirmation, expected revision, and a valid ASCII idempotency key.
- Store the receipt and audit through the control command store; replay the exact receipt for the same key/input and reject changed input. Hosted production uses PostgreSQL; local authority mode can recover an ambiguous filesystem quarantine from the verified resource record after restart.
- If the process stops after the filesystem quarantine rename but before the idempotency receipt is committed, a retry may recover the receipt only by validating the deterministic quarantine entry for the same profile ID and revision.
- A different or corrupt quarantine entry is an error; never infer success from a missing active profile alone.

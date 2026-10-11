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

- Hosted permanent delete needs quarantine plus object-retention cleanup proof.
- Proxy credentials need a workspace encryption-envelope authority before hosted persistence.

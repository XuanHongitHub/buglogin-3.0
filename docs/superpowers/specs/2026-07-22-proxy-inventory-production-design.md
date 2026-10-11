# Proxy Inventory Production Design

Status: implementation wave; Rust authority remains canonical.

## Scope

Promote the existing Rust proxy repository from local-only helper to hosted control-plane authority. Preserve fast parsing, credential-presence metadata only, workspace isolation, CAS revisions, assignment integrity, and redacted responses.

## Commands

- `proxy.parse`: existing query; raw text input; normalized redacted report.
- `proxy.list`: workspace-scoped query; returns proxy metadata and revision, never credential values.
- `proxy.create`: workspace-scoped mutation; endpoint dedupe; no credential material or reference accepted.
- `proxy.update`: workspace-scoped CAS mutation; endpoint dedupe; no credential material or reference accepted.
- `proxy.delete`: workspace-scoped CAS mutation; rejects assigned proxy.
Assignment remains out of this wave. Existing `Profile.proxy_id` is the current projection; a separate assignment repository is not persisted until an ADR resolves dual authority.

All mutations require authentication, server ACL, audit, idempotency, and stable problem responses. All reads require authentication and server ACL. MCP/REST/Tauri adapters derive from the same registry metadata.

## Persistence

Hosted proxy records persist inside the canonical control aggregate. Restore validates workspace IDs, proxy IDs, revisions, endpoint uniqueness, and rebuilds indexes. Corrupt state fails closed before mutation. Assignment persistence is deferred until authority is resolved.

## Security

Proxy credentials remain in OS-vault-backed credential objects. Inventory DTOs expose only `hasCredential`; they never expose `credentialRef`, username, password, or rotation URLs. No UI-only permission checks.

## Non-scope

Health probing, rotation jobs, VPN/DNS, credential upload/rotation, and web visual redesign remain separate waves.

## Acceptance

- Hosted restart preserves inventory; assignment persistence remains deferred.
- Cross-workspace reads/writes fail closed.
- Duplicate endpoint create deduplicates deterministically.
- Stale revisions do not mutate state.
- Metadata CRUD survives hosted restart.
- REST, MCP, Tauri, and desktop production paths return the same canonical DTOs.
- Credential material is not accepted in this wave; only existing vault-backed credential presence is exposed as `hasCredential`.



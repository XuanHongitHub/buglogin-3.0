# ADR-001: Desktop Core Process Topology

- Status: Accepted for v1
- Date: 2026-07-20

## Decision

Use a desktop-first Tauri 2 shell with React as a projection and one Rust application core. The core owns filesystem access, child processes, networking, secrets, permissions, lifecycle state, local REST, MCP, diagnostics, and updates.

```text
React UI --typed Tauri commands/events--> Rust core
                                         |-- profiles/runtime/network
                                         |-- local MCP/Tauri command registry
                                         |-- local files + protected secrets
                                         `-- optional sync server/object storage
```

Tauri commands validate input, invoke one domain operation, and return typed result/error. Long operations expose operation IDs, progress, cancellation policy, timeout, retry policy, and terminal state. UI state never authorizes an operation.

## Consequences

- `local` profiles may launch offline only in the `local` self-hosted artifact.
- The `production` artifact never starts a local control server, opens a local metadata repository, or falls back when hosted API access fails.
- In `production`, metadata, permissions, proxy configuration, manifests, profile mutations, audit, and sync state cross the hosted HTTPS API. Local disk contains only replaceable encrypted projections, staging, and runtime binaries.
- Every managed/shared profile has one canonical `ProfileResource` binding containing `profileId`, `resourceId`, and `workspaceId`; lease and sync scope derive only from this binding.
- Local profiles may launch offline without a remote lease. Managed/shared profiles fail closed for write launch when their binding or lease authority is unavailable; read-only inspection remains allowed.
- Lease tokens stay inside Rust process memory. React, REST clients, and MCP callers never receive or submit lease tokens.
- Hosted REST, MCP, Tauri, and UI share the Rust command registry and ACL/audit path. Local mode may expose the same registry through its self-hosted authority.
- Runtime, process, and filesystem failures stay outside React.
- Hosted control plane is mandatory for `production`; local authority is mandatory for `local`.

## Rejected

- NestJS/web server as the desktop business authority.
- Page components owning side effects or process state.
- UI-only permission enforcement.

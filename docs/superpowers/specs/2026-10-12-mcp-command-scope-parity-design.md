# MCP Command Scope Parity Design

Status: implemented and locally verified.

## Problem

MCP authorization derives from the canonical command registry, but each command's resource scope is resolved separately. A missing mapping falls back to global scope. For commands whose permission is granted at workspace scope, this can deny valid users before the executor's service check. The `team.create` executor requires `workspaceId`, while the resolver omitted its workspace scope.

## Contract

- Every MCP command with a resource-scoped authorization requirement must resolve the same canonical scope its executor/service uses.
- Missing resource scope must fail validation or resolve from authoritative bindings; it must not silently degrade to global scope.
- Existing server-side service authorization remains authoritative after generic MCP policy authorization.
- Fixes must not widen grants or infer tenant IDs from caller-owned resource IDs when an authoritative binding exists.

## Verification

- Add a regression proving a workspace-scoped `CAP-TEAM-001` create grant authorizes `team.create` only when its `workspaceId` is resolved.
- Run MCP execution and vertical test suites, command-registry generation check, formatting, and diff validation.

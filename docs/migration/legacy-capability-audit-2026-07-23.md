# Legacy Capability Audit — 2026-07-23

## Scope

Read-only comparison of `E:\bug-login` and `.tmp/buglogin-legacy`. No legacy files, production data, logs, or secrets were modified or read for content.

## Source identity

| Check | Result |
|---|---|
| Legacy commit | `81f9af637825c79c7ca60bbbdf2c11b4fecfa310` |
| Snapshot commit | `81f9af637825c79c7ca60bbbdf2c11b4fecfa310` |
| Git tree | `c4810cdc71599fae26ccd5ff4611cbc4008d2245` |
| Tracked files | `2,047` on both sides |
| Tracked source drift | `0` |
| Audit command | `node scripts/migration/audit-legacy.mjs` |

The snapshot is a valid tracked-source reference. It is **not** production-data evidence and does not represent untracked legacy runtime state.

## Legacy inventory

The audit found `209` Tauri command definitions and `96` web route/layout files. File counts are inventory signals only; they are not parity proof.

## Disposition matrix

| Domain | Legacy behavior | BugLogin 2.0 status | Release disposition |
|---|---|---|---|
| Profile lifecycle | Create/list/launch/stop/clone/import | Core list/create/launch/stop/archive/restore; metadata-only clone authority | `replace`; import and desktop clone UI remain open |
| Proxy | Parse/CRUD/check/benchmark/import/export/assignment | Parse + metadata CRUD | `replace`; egress, health, batch, import/export missing |
| Cookies | Read/import/export/bulk/copy | Vault-safe crate logic only | `replace`; REST/Tauri/MCP/UI/sync exposure missing |
| Profile copy/clone | Existing legacy workflows | Canonical hosted REST/MCP metadata-only clone with revision/ACL/idempotency path | `replace`; desktop copy-data workflow remains open |
| Sharing | Group/share/lock workflows | Canonical hosted REST/MCP share/revoke mapped to resource grants | `replace`; desktop invitation/acceptance/lock UX remains open |
| Runtimes | Catalog/download/update and legacy engines | Wayfern launch path only | `adapter` or `unsupported`; each runtime needs explicit approval |
| Extensions | Package/group/assignment/load | Migration package mapping only | `replace`; lifecycle and load evidence missing |
| VPN | Import/CRUD/check/connect/status | Migration partial only | `partial`; command/UI/MCP path missing |
| Sync | Settings, pull/recover, conflict handling | Manifest/blob/lease primitives | `replace`; hosted interruption/reconnect evidence missing |
| Workspace/team/member/resource | Portal/admin lifecycle | Basic CRUD/invite/suspend/role + resource lease | `replace`; delete/archive/hierarchy/full ACL management missing |
| Admin | Users, permissions, audit, jobs, incidents, support, governance | Capability declarations and small summary UI | `replace`; canonical admin command/UI surface missing |
| HR | Employee lifecycle | Deliberately `later` | `later`; requires legal/schema approval |
| Desktop operations | Updates, diagnostics, traffic, settings, quit | Partial shell | `replace`; command parity incomplete |
| MCP | Legacy start/stop/config | Registry-driven ACL/MCP foundation | `replace`; absent commands cannot be MCP-enabled |

## Current P0 blockers

1. Real production data-root inspect and immutable snapshot.
2. Two production-clone rehearsals with exact counts, hashes, ACL, profile-resource graph, rollback, and repeat import.
3. Full `MIG-002`: migrate, launch, cookies, proxy, extensions.
4. Trusted-TLS PostgreSQL, Redis, and S3 evidence.
5. Desktop cookie, clone/copy/share, runtime, extension, VPN, and sync workflows.
6. Real proxy egress and network interruption/reconnect tests.
7. Signed Windows evidence plus native macOS arm64 and Intel/waiver evidence.
8. Hosted auth/sync E2E, immutable Git provenance, and release approval package.

## Recent correction

Local `profile.create` now persists `Ready`, matching the local launch contract. Regression coverage verifies create returns `ready`; desktop library tests pass `55/55`.

Production migration remains blocked until the P0 evidence and capability gaps above close.


## Production data-root correction — 2026-07-23

The repository `E:\bug-login` is source code, not the active local profile data root. The active Windows data root is `%LOCALAPPDATA%\BugLogin`. Two snapshots matched at `9da4587dead42f0dc4e0a22c25196338292b15ad06ae7fddfcc4a12990013912`.

The corrected inspector recognizes legacy `browser` metadata and canonical profile paths. It found `15` profiles, all `bugmini`, and the historical result returned `cutoverEligibility: blocked` with `runtime_adapter_not_approved`. No data was modified.

## Status correction — 2026-08-21 (current wave)

The `bugmini` adapter is now implemented in Rust and selected by API lifecycle dispatch. The historical report remains valid evidence of the source state; the adapter blocker is now represented as a partial runtime gate until artifact verification completes. Native Windows/macOS evidence, control-plane export, and production-clone rehearsal/rollback remain open migration gates.

## Status correction — 2026-08-21

Hosted profile metadata-only clone and profile share/revoke now have canonical domain inputs/receipts, REST routes, MCP dispatch, OpenAPI schemas, generated command metadata, path-identity validation, revision checks, ACL checks, audit/idempotency through the shared mutation path, and targeted tests. This does not close desktop clone/copy/share UX, cookie authority durability, legacy control-plane export, or release gates.

# ADR-004: Explicit Trust Boundaries

- Status: Accepted for v1
- Date: 2026-07-20

## Decision

Treat the React UI, local REST, MCP, runtime binaries, browser profile data, sync service, object storage, imported legacy data, and updater as separate trust zones. Rust is the policy and capability enforcement point.

Local REST/MCP bind to localhost, stay disabled by default where possible, require a random bearer token with rotation/revocation, and expose only scoped commands. Every adapter resolves identity, workspace/resource scope, capability, idempotency, and audit before execution. Tauri uses the same command registry and policy path.

Validate paths against approved roots; reject traversal and symlink escape where relevant. Verify runtime/update checksums and signatures before install. Redact secrets from errors, logs, analytics, diagnostics, and UI snapshots. Imported data is immutable source material; migration never deletes source data.

## Consequences

- Browser automation cannot bypass ACLs through MCP or local API.
- Runtime and update artifacts require provenance verification.
- Secret handling is centralized and auditable.
- Remote exposure requires a new threat model and explicit approval.

## Rejected

- UI checks as a security boundary.
- Anonymous local API/MCP.
- Remote API exposure by default.
- MITM/custom CA rewriting in v1.

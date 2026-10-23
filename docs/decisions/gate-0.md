# Gate 0 Decision Record

- Date: 2026-07-20
- Status: approved baseline for implementation readiness

## Product

- Primary persona: multi-account operators, agencies, and teams managing isolated browser profiles and shared operational resources.
- Product shape: desktop browser operations + public/account/workspace/admin web + hosted control/sync plane.
- Core capabilities: profiles, Wayfern and legacy runtimes, fast proxy parse/check, cookies/extensions/import/clone/share, workspace/team/member/resource ACL, sync, migration, REST/MCP, admin/audit.
- UI direction: complete redesign; legacy UI/routes are evidence only, not implementation targets.
- Languages: Vietnamese and English launch baseline.

## Platforms

- Required: Windows x64, macOS Apple Silicon, macOS Intel while upstream/runtime artifacts support it.
- Linux: later; not a launch blocker.
- Every platform-sensitive change requires Windows/macOS evidence.

## Runtime policy

- Wayfern is primary.
- Runtime taxonomy is canonicalized as `wayfern`, `bugox`, `bugium`, `bugmini`, `bugthor`, `bughelium`; upstream aliases `cloak`, `thorium`, and `helium` are adapter input aliases only. See `docs/contracts/runtime-taxonomy-v1.md`.
- Capability parity is explicit; unsupported config is not silently converted.
- Runtime artifacts require checksum/signature verification, version pinning, rollback, and capability metadata.

## Architecture

- Authority language: Rust.
- UI language: TypeScript + React.
- Desktop: Tauri 2.
- Local metadata: upstream-compatible versioned JSON/filesystem first, behind a repository boundary; SQLite is allowed only after measured limits and an approved migration. Browser profile payloads remain filesystem data.
- Hosted authority: Rust control server + PostgreSQL.
- Redis: ephemeral locks, presence, rate limits, and short-lived jobs only; never durable source of truth.
- Profile/blob sync: S3-compatible object storage with versioned manifests, hashes, encryption, resumability, tombstones.
- Next may remain frontend tooling/static export; no business authority in Next routes/server actions.
- No NestJS in BugLogin 2.0.

## Security and access

- Rust control/domain layer is the single permission authority.
- Resource/capability ACL applies identically to desktop, web, REST, and MCP.
- Local API binds loopback and requires scoped token.
- Hosted APIs require TLS and server-managed sessions/tokens.
- MCP exposes all approved capabilities through the same auth, audit, confirmation, and redaction policies.
- Secrets never enter normal logs, screenshots, fixtures, or telemetry.

## Feature access

- Approved BugLogin core capabilities are open; no local PRO paywall.
- Removing paywalls does not remove security, data-loss, credential, signature, or concurrency protections.
- Billing/commerce is a separate module; local profile access never depends on billing availability.

## Migration

- Source data remains untouched.
- Inspect, snapshot, dry-run, neutral export, idempotent import, verify, cutover, rollback.
- New database, object namespace, data root, and key namespace.
- First production cutover uses a short write freeze; no initial live dual-write.
- Minimum 30-day recoverability window, adjustable by policy.

## Licensing and acceptable use

- Development baseline assumes AGPL-3.0 compliance for the Donut-derived code.
- Public/commercial distribution remains gated by formal license, attribution, trademark, and dependency review.
- Acceptable use excludes credential theft, unauthorized account access, malicious automation, and bypassing security controls owned by third parties.

## Deferred launch scope

- Linux.
- Payroll/sensitive HR records.
- OpenVPN unless later approved.
- Microservices/Kafka.
- Live dual-write migration.
- Unregistered browser automation outside the command registry.


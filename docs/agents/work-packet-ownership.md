# BugLogin 50 Work-Packet Ownership Manifest

- Version: `1.1.0`; updated `2026-08-11`
- Future implementation only. Registry: `docs/capabilities/registry.md`.
- One active writer per exact path scope. Shared contracts change only through the named integration owner.

## Owners

`IO-ARCH` architecture/domain/contracts; `IO-CORE` desktop/browser/network; `IO-CONTROL` identity/control/ACL/audit; `IO-DATA` storage/sync/migration; `IO-UI` frontend/design; `IO-AUTO` REST/OpenAPI/MCP; `IO-RELEASE` OS/CI/signing/updater; `IO-QA` E2E/security/performance/release evidence.

## Acceptance Commands

| ID | Exact command |
|---|---|
| `C-FMT` | `cargo fmt --all -- --check && pnpm exec biome check .` |
| `C-DOMAIN` | `cargo test -p buglogin-domain` |
| `C-BROWSER` | `cargo test -p buglogin-browser` |
| `C-NETWORK` | `cargo test -p buglogin-network` |
| `C-STORAGE` | `cargo test -p buglogin-storage` |
| `C-SYNC` | `cargo test -p buglogin-sync` |
| `C-API` | `cargo test -p buglogin-api` |
| `C-MIGRATION` | `cargo test -p buglogin-migration` |
| `C-CONTROL` | `cargo test -p buglogin-control-app` |
| `C-DESKTOP` | `pnpm --filter @buglogin/desktop test && pnpm --filter @buglogin/desktop typecheck` |
| `C-WEB` | `pnpm --filter @buglogin/web test && pnpm --filter @buglogin/web typecheck` |
| `C-CLIENT` | `pnpm generate:api && git diff --exit-code -- frontend/src/generated` |
| `C-OPENAPI` | `cargo test -p buglogin-api --test openapi_coverage && pnpm test:contract` |
| `C-MCP` | `cargo test -p buglogin-api mcp_` |
| `C-E2E-VERTICAL` | `pnpm e2e --project=vertical-slice` |
| `C-E2E-DESKTOP` | `pnpm e2e --project=desktop` |
| `C-E2E-WEB` | `pnpm e2e --project=web` |
| `C-E2E-ROLES` | `pnpm e2e --project=role-matrix` |
| `C-E2E-SYNC` | `pnpm e2e --project=sync-interruption` |
| `C-E2E-MIGRATION` | `pnpm e2e --project=migration-rollback` |
| `C-E2E-WIN` | `pnpm e2e --project=windows-smoke` |
| `C-E2E-MAC` | `pnpm e2e --project=macos-smoke` |
| `C-A11Y` | `pnpm test:a11y` |
| `C-VISUAL` | `pnpm test:visual` |
| `C-SECURITY` | `cargo audit && pnpm audit --prod && pnpm test:security` |
| `C-PERF` | `pnpm test:performance` |
| `C-RELEASE` | `pnpm release:verify` |
| `C-DOCS` | `pnpm docs:validate` |

## Universal Stop Conditions

Stop on missing authority, overlapping writer, duplicate DTO/permission/type, mock production behavior, frontend-only ACL, source mutation, secret leakage, nondeterministic migration, missing OS evidence, failed dependency/gate, or foreign-scope failure. Report; never patch another packet.

### UI packet gate

`IO-UI` and any packet that changes a user-facing screen must read `docs/design/ui-authority.md`, `DESIGN.md`, the relevant IA row, and the state matrix before editing. Desktop work must inspect the pinned Donut source at the manifest commit and reuse its shell/component/theme language. Web may change density by surface mode but must use the shared semantic language. Static mockups, fake records, simulated progress, and one-off raw-CSS shells are never production evidence. `C-VISUAL` and `C-A11Y` are required before a UI packet is accepted; missing reference or conflicting visual authority is a stop condition.

## Manifest

| Packet | Capability | Exact intended future path scope | Dependencies | Gate | Acceptance | Packet stop | Owner |
|---|---|---|---|---|---|---|---|
| `WP-01` | `CAP-DOM-001` IDs/entities | `crates/domain/src/ids/**`; `crates/domain/src/entities/**`; `crates/domain/tests/ids.rs` | none | G0 contract → stable serialization | `C-DOMAIN`, `C-FMT` | second canonical ID | IO-ARCH |
| `WP-02` | `CAP-SEC-001` permission model | `crates/domain/src/permissions/**`; `crates/domain/tests/permission_matrix.rs` | WP-01 | G0 → deny-by-default matrix | `C-DOMAIN`, `C-E2E-ROLES` | UI/API role split | IO-CONTROL |
| `WP-03` | `CAP-DOM-001` states/errors | `crates/domain/src/state/**`; `crates/domain/src/errors.rs`; `crates/domain/tests/state_transitions.rs` | WP-01 | G0 → explicit transitions/errors | `C-DOMAIN`, `C-FMT` | UI-owned transition | IO-ARCH |
| `WP-04` | `CAP-API-001` command registry | `crates/api/src/commands/registry.rs`; `crates/api/src/commands/schema.rs`; `crates/api/tests/command_registry.rs`; `docs/contracts/command-registry-v1.yaml` | WP-01..03 | G0 → IDs, ACL, audit, idempotency metadata | `C-API`, `C-OPENAPI` | route bypasses service | IO-AUTO |
| `WP-05` | `CAP-DOM-001` Rust workspace | `Cargo.toml`; `Cargo.lock`; `rust-toolchain.toml`; `crates/*/Cargo.toml`; `apps/control/Cargo.toml`; `apps/desktop/src-tauri/Cargo.toml` | WP-01..04 | G0 → empty workspace compiles | `cargo check --workspace`, `C-FMT` | unapproved dependency/behavior | IO-ARCH |
| `WP-06` | `CAP-STOR-001` local repository | `crates/storage/src/local/**`; `crates/storage/src/lib.rs`; `crates/storage/tests/local_repository.rs` | WP-01, WP-03, WP-05 | G1 → atomic versioned repository | `C-STORAGE`, `C-FMT` | dual JSON/DB writes | IO-DATA |
| `WP-07` | `CAP-STOR-001/002` path/blob safety | `crates/storage/src/blob/**`; `crates/storage/src/paths/**`; `crates/storage/tests/blob_integrity.rs`; `crates/storage/tests/path_safety.rs` | WP-06 | G1 → checksum/root/traversal defense | `C-STORAGE`, `C-SECURITY` | raw caller path | IO-DATA |
| `WP-08` | `CAP-API-002` control skeleton | `apps/control/src/**`; `apps/control/tests/health.rs`; `apps/control/config/**` | WP-04, WP-05 | G1 → one binary/health/shutdown | `C-CONTROL`, `C-API` | business logic in routes | IO-CONTROL |
| `WP-09` | `CAP-API-002` frontend/client | `pnpm-workspace.yaml`; root `package.json`; `frontend/package.json`; `frontend/src/generated/**`; `scripts/generate-api.*` | WP-04, WP-05 | G1 → deterministic generated client | `C-CLIENT`, `C-FMT` | handwritten generated DTO | IO-UI |
| `WP-10` | UI foundations | `frontend/src/design-system/tokens/**`; `frontend/src/design-system/primitives/**`; `frontend/src/design-system/index.ts`; tests beside primitives | WP-09 | G1 → accessible shared primitives | `C-WEB`, `C-A11Y`, `C-VISUAL` | feature-specific primitive fork | IO-UI |
| `WP-11` | E2E fixtures | `tests/e2e/config/**`; `tests/e2e/fixtures/**`; `tests/e2e/support/**`; `docker/e2e/**`; `playwright.config.ts` | WP-05, WP-08, WP-09 | G1 → two users/teams/workspaces + legacy fixtures | `pnpm e2e:fixtures:verify`, `C-E2E-VERTICAL` | production credentials/data | IO-QA |
| `WP-12` | `CAP-REL-001` CI | `.github/workflows/ci.yml`; `.github/workflows/contracts.yml`; `.github/actions/setup-buglogin/**`; `scripts/ci/**` | WP-05, WP-09, WP-11 | G1 → scoped checks/secret boundaries | `pnpm ci:verify`, `C-SECURITY` | signing secret in PR job | IO-RELEASE |
| `WP-13` | `CAP-ID-001/002` identity | `crates/domain/src/identity/**`; `crates/api/src/auth/**`; `apps/control/tests/auth.rs` | WP-02, WP-04, WP-06, WP-08 | G2 → auth/session/revoke/audit | `C-DOMAIN`, `C-API`, `C-SECURITY` | plaintext token or URL secret | IO-CONTROL |
| `WP-14` | `CAP-ORG-001/WS-001` tenancy | `crates/domain/src/organization/**`; `crates/domain/src/workspace/**`; `crates/api/src/services/workspaces/**`; `apps/control/tests/workspaces.rs` | WP-13, WP-02 | G2 → tenant isolation + policy | `C-DOMAIN`, `C-API`, `C-E2E-ROLES` | offline profiles entitlement-gated | IO-CONTROL |
| `WP-15` | `CAP-PROF-001/RES-001` profile slice | `crates/domain/src/profile/**`; `crates/browser/src/profile/**`; `crates/browser/tests/profile_crud.rs` | WP-06, WP-14 | G2 → real CRUD/archive/resource ID | `C-BROWSER`, `C-STORAGE` | unsafe metadata/data transaction | IO-CORE |
| `WP-16` | `CAP-NET-001/SEC-002` parser | `crates/network/src/proxy/model.rs`; `crates/network/src/proxy/parser/**`; `crates/network/tests/proxy_parser.rs` | WP-03, WP-07 | G2 → formats/dedupe/redaction | `C-NETWORK`, `C-SECURITY` | parse performs network check/logs secret | IO-CORE |
| `WP-17` | `CAP-RUN-001/003` runtime slice | `crates/browser/src/runtime/contract.rs`; `crates/browser/src/runtime/wayfern/**`; `crates/browser/tests/runtime_contract.rs` | WP-03, WP-07, WP-15 | G2 → one approved runtime adapter | `C-BROWSER`, `C-E2E-WIN`, `C-E2E-MAC` | second engine branches core | IO-CORE |
| `WP-18` | `CAP-PROF-002/DESK-004` lifecycle | `crates/browser/src/lifecycle/**`; `crates/browser/src/process/**`; `crates/browser/tests/lifecycle.rs` | WP-16, WP-17 | G2 → launch/stop/status/crash cleanup | `C-BROWSER`, `C-E2E-DESKTOP` | discover tree after parent death | IO-CORE |
| `WP-19` | `CAP-API-002` REST/OpenAPI slice | `crates/api/src/http/**`; `crates/api/openapi/**`; `crates/api/tests/openapi_coverage.rs` | WP-04, WP-13..18 | G2 → auth/ACL/audit/idempotency | `C-API`, `C-OPENAPI`, `C-CLIENT` | route owns domain decision | IO-AUTO |
| `WP-20` | `CAP-MCP-001` MCP slice | `crates/api/src/mcp/**`; `crates/api/tests/mcp_vertical.rs` | WP-04, WP-13, WP-19 | G2 → opt-in/scoped/audited | `C-MCP`, `C-E2E-VERTICAL` | secret output/registry bypass | IO-AUTO |
| `WP-21` | desktop vertical slice | `apps/desktop/src-tauri/**`; `frontend/src/apps/desktop/shell/**`; `frontend/src/apps/desktop/features/vertical-slice/**` | WP-09, WP-10, WP-13..20 | G2 → real end-to-end journey | `C-DESKTOP`, `C-E2E-VERTICAL`, `C-A11Y`, `C-VISUAL` | mocks/stubs/frontend ACL | IO-UI |
| `WP-22` | `CAP-PROF-003` groups/bulk | `crates/browser/src/profile/groups/**`; `crates/browser/src/profile/bulk/**`; `crates/browser/tests/profile_bulk.rs` | WP-15, WP-02 | G3 → partial-result semantics | `C-BROWSER`, `C-E2E-ROLES` | silently skip denied target | IO-CORE |
| `WP-23` | `CAP-RUN-001/002` catalog/download | `crates/browser/src/runtime/catalog/**`; `crates/browser/src/runtime/download/**`; `crates/browser/tests/runtime_download.rs` | WP-17, WP-45 review | G3 → signed/checksummed rollback | `C-BROWSER`, `C-SECURITY` | unsigned asset activates | IO-CORE |
| `WP-24` | `CAP-RUN-004` legacy adapters | `crates/browser/src/runtime/legacy/**`; `crates/browser/tests/legacy_runtime_matrix.rs` | WP-17; approved later gate | G3 → explicit supported matrix | `C-BROWSER`, `C-E2E-WIN`, `C-E2E-MAC` | no measured need/legal/signing | IO-CORE |
| `WP-25` | `CAP-NET-001` proxy inventory | `crates/network/src/proxy/repository/**`; `crates/network/src/proxy/assignment/**`; `crates/network/tests/proxy_assignment.rs` | WP-16, WP-14 | G3 → grants/assignment integrity | `C-NETWORK`, `C-E2E-ROLES` | proxy use implies secret-read | IO-CORE |
| `WP-26` | `CAP-NET-002/JOB-001` proxy jobs | `crates/network/src/proxy/health/**`; `crates/domain/src/jobs/**`; `crates/network/tests/proxy_health.rs`; `apps/desktop/src-tauri/src/{proxy_bridge,proxy_store,adapter,desktop_state,lib}.rs`; `apps/desktop/src/{bridge.ts,contracts.ts,bridge.self-test.ts}`; proxy-check rows in `docs/contracts/command-registry-v1.yaml`, `crates/api/openapi/openapi-v1.yaml`, and `docs/capabilities/registry.md`; generated API artifacts | WP-25, WP-04 | G3 → bounded progress/cancel/partial; desktop probe uses vault-held credentials and server-authorized proxy revision; REST/MCP batch checks wait for connected-device dispatch | `C-NETWORK`, `C-PERF`, `C-SECURITY` | unbounded concurrency, parse waits, secret leaves device, or false healthy result | IO-CORE |
| `WP-27` | `CAP-NET-003/004` VPN/DNS | `crates/network/src/vpn/**`; `crates/network/src/dns/**`; `crates/network/tests/vpn_dns.rs` | WP-18; approved later gates | G3 → privilege/cleanup/exclusion | `C-NETWORK`, `C-E2E-WIN`, `C-E2E-MAC` | no approved need/OS evidence | IO-CORE |
| `WP-28` | `CAP-COOK-001` cookies | `crates/browser/src/cookies/**`; `crates/browser/tests/cookies.rs` | WP-15, WP-18, WP-32 | G3 → controlled persistence/redaction | `C-BROWSER`, `C-SECURITY` | values in logs/audit/MCP | IO-CORE |
| `WP-29` | `CAP-EXT-001` extension policy | `crates/browser/src/extensions/policy/**`; `crates/browser/src/extensions/package/**`; `crates/browser/tests/extension_policy.rs` | WP-07, WP-32, WP-45 | G3 → source/hash/signature/quarantine | `C-BROWSER`, `C-SECURITY` | opaque bundled extension | IO-CORE |
| `WP-30` | `CAP-EXT-002` extension lifecycle | `crates/browser/src/extensions/lifecycle/**`; `crates/browser/src/extensions/assignment/**`; `crates/browser/tests/extensions.rs` | WP-22, WP-29 | G3 → install/update/remove/rollback | `C-BROWSER`, `C-E2E-DESKTOP` | no rollback or policy bypass | IO-CORE |
| `WP-31` | `CAP-TEAM-001/MEM-001` people | `crates/domain/src/team/**`; `crates/domain/src/member/**`; `crates/api/src/services/members/**`; `apps/control/tests/members.rs` | WP-14, WP-02 | G3 → invite/suspend/roles/boundaries | `C-DOMAIN`, `C-API`, `C-E2E-ROLES` | escalation/reusable invite secret | IO-CONTROL |
| `WP-32` | `CAP-SEC-002` secrets | `crates/storage/src/secrets/**`; `crates/storage/tests/secrets.rs` | WP-07, WP-13, WP-44 contract | G3 → OS vault/redaction/rotation | `C-STORAGE`, `C-SECURITY`, `C-E2E-WIN`, `C-E2E-MAC` | plaintext fallback | IO-DATA |
| `WP-33` | `CAP-RES-001/SHARE-001` resources/grants | `crates/domain/src/resource/**`; `crates/domain/src/grant/**`; `crates/api/src/services/resources/**`; `apps/control/tests/resource_grants.rs` | WP-02, WP-14, WP-31 | G3 → typed grant/revoke/tenant lookup | `C-DOMAIN`, `C-API`, `C-E2E-ROLES` | stringly permission model | IO-CONTROL |
| `WP-34` | `CAP-SHARE-002` leases/locks | `crates/domain/src/lease/**`; `crates/api/src/services/leases/**`; `crates/sync/src/coordination/**`; `apps/control/tests/leases.rs`; `crates/sync/tests/coordination.rs` | WP-18, WP-33, WP-36 | G3 → expiry/partition/stale recovery | `C-DOMAIN`, `C-API`, `C-SYNC`, `C-E2E-SYNC` | network loss permanently locks local data | IO-CONTROL |
| `WP-35` | `CAP-AUD-001` audit/support | `crates/domain/src/audit/**`; `crates/api/src/services/audit/**`; `apps/control/tests/audit.rs` | WP-02, WP-13, WP-33 | G3 → append-only/redacted/expiring support | `C-DOMAIN`, `C-API`, `C-SECURITY` | missing actor/resource/command correlation | IO-CONTROL |
| `WP-36` | `CAP-SYNC-001` manifests/conflicts | `crates/sync/src/manifest/**`; `crates/sync/src/conflict/**`; `crates/sync/tests/manifests.rs` | WP-06, WP-07, WP-33 | G3 → versions/conflicts/idempotent retry | `C-SYNC`, `C-E2E-SYNC` | silent last-write data loss | IO-DATA |
| `WP-37` | `CAP-SYNC-002/STOR-002` encrypted chunks | `crates/sync/src/crypto/**`; `crates/sync/src/chunks/**`; `crates/storage/src/remote/**`; `crates/sync/tests/encryption.rs` | WP-32, WP-36 | G3 → client keys/checksums/recovery | `C-SYNC`, `C-STORAGE`, `C-SECURITY` | server derives plaintext key | IO-DATA |
| `WP-38` | `CAP-STOR-002/DIAG-001` backup | `crates/storage/src/backup/**`; `crates/storage/tests/backup_restore.rs` | WP-07, WP-32, WP-37 | G3 → immutable verified restore | `C-STORAGE`, `C-E2E-MIGRATION` | overwrite without snapshot/confirm | IO-DATA |
| `WP-39` | `CAP-MIG-001` inspect/snapshot | `crates/migration/src/inspect/**`; `crates/migration/src/snapshot/**`; `crates/migration/src/package/**`; `crates/migration/tests/inspect.rs` | WP-01, WP-06, WP-07, WP-11 | G3 → read-only deterministic package | `C-MIGRATION`, `C-SECURITY` | source mutation/guessed path/secret report | IO-DATA |
| `WP-40` | `CAP-MIG-002` cutover/rollback | `crates/migration/src/transform/**`; `crates/migration/src/verify/**`; `crates/migration/src/cutover/**`; `crates/migration/src/rollback/**`; `crates/migration/tests/cutover.rs` | WP-23, WP-25, WP-28..39 | G3 → classify all; rehearse rollback twice | `C-MIGRATION`, `C-E2E-MIGRATION` | any registry migration stop | IO-DATA |
| `WP-41` | Desktop breadth UI | `frontend/src/apps/desktop/features/profiles/**`; `resources/**`; `jobs/**`; `settings/**` under desktop | WP-21..40, WP-10 | G3 → real states for all desktop jobs | `C-DESKTOP`, `C-A11Y`, `C-VISUAL` | hidden errors/unhandled offline/partial | IO-UI |
| `WP-42` | `CAP-WEB-002/003` account/workspace UI | `frontend/src/apps/web/account/**`; `frontend/src/apps/web/workspace/**`; `frontend/src/apps/web/router.tsx` | WP-13, WP-14, WP-31..37, WP-09, WP-10 | G3 → generated client, real operations | `C-WEB`, `C-E2E-WEB`, `C-E2E-ROLES`, `C-A11Y` | mutation lacks server ACL/audit | IO-UI |
| `WP-43` | `CAP-WEB-001/004/ADM-001` public/admin UI | `frontend/src/apps/web/public/**`; `frontend/src/apps/web/admin/**`; `frontend/src/apps/web/shells/**` | WP-10, WP-23, WP-35, WP-42 | G3 → public/download/status + isolated admin | `C-WEB`, `C-E2E-WEB`, `C-A11Y`, `C-VISUAL` | billing/HR smuggled or admin leak | IO-UI |
| `WP-44` | `CAP-PLAT-001/DESK-003/004` OS | `crates/browser/src/platform/windows/**`; `crates/browser/src/platform/macos/**`; `apps/desktop/src-tauri/src/platform/**`; `tests/platform/**` | WP-18, WP-21, WP-32 | G4 → vault/deep-link/tray/cleanup both OSes | `C-E2E-WIN`, `C-E2E-MAC`, `C-BROWSER` | changed OS lacks VM/physical evidence | IO-RELEASE |
| `WP-45` | `CAP-REL-002` release catalogs | `crates/domain/src/release/**`; `crates/api/src/services/releases/**`; `apps/control/tests/releases.rs` | WP-02, WP-35, WP-44 | G4 → dual-controlled signed publish/rollback | `C-DOMAIN`, `C-API`, `C-SECURITY` | one actor publishes unverified artifact | IO-RELEASE |
| `WP-46` | `CAP-REL-001/003` signing/updater | `.github/workflows/release.yml`; `.github/workflows/signing.yml`; `scripts/release/**`; `apps/desktop/src-tauri/src/updater/**`; `tests/release/**` | WP-12, WP-23, WP-44, WP-45 | G4 → SBOM/provenance/interruption/rollback | `C-RELEASE`, `C-E2E-WIN`, `C-E2E-MAC`, `C-SECURITY` | secret in PR job/data-mutating rollback | IO-RELEASE |
| `WP-47` | API/MCP compatibility | `tests/contracts/**`; `crates/api/tests/compatibility/**`; `frontend/src/generated/**`; `docs/contracts/compatibility.md` | WP-19, WP-20, all domain packets | G4 breadth freeze → interface parity | `C-OPENAPI`, `C-MCP`, `C-CLIENT`, `C-E2E-VERTICAL` | duplicate DTO/command bypass | IO-AUTO |
| `WP-48` | Full E2E/visual QA | `tests/e2e/specs/**`; `tests/e2e/pages/**`; `tests/visual/**`; `artifacts/e2e/.gitkeep` | WP-40..47 | G4 feature freeze → full journeys pass | `C-E2E-DESKTOP`, `C-E2E-WEB`, `C-E2E-ROLES`, `C-E2E-SYNC`, `C-E2E-MIGRATION`, `C-A11Y`, `C-VISUAL` | breadth changes while failures remain | IO-QA |
| `WP-49` | Security/performance review | `tests/security/**`; `tests/performance/**`; `docs/reviews/security/**`; `docs/reviews/performance/**` | WP-47, WP-48 | G5 → threats/ACL/load/residual risks approved | `C-SECURITY`, `C-PERF`, `C-E2E-ROLES` | reviewer patches foreign code | IO-QA |
| `WP-50` | Migration/release readiness | `docs/runbooks/**`; `docs/release/**`; `docs/migration/**`; `tests/rehearsal/**` | WP-40, WP-46, WP-48, WP-49 | G5 → two rehearsals, rollback, support, signed risks | `C-E2E-MIGRATION`, `C-RELEASE`, `C-DOCS` | source/hash/ACL/secret/old-write/rollback/critical launch failure | IO-QA |

## Handoff

Each packet returns owned paths, capability IDs, contract changes, commands/results, evidence paths, unresolved risks, stop conditions, integration-owner decisions. Isolated tests do not close a packet when its gate or vertical journey fails.


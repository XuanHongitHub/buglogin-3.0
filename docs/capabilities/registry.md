# BugLogin Capability Registry

- Registry version: `1.0.0`
- Status: architecture contract; implementation not started
- Updated: `2026-07-20`
- Authority: capability IDs, disposition, dependencies, access, cross-surface impact
- Source: `docs/superpowers/specs/2026-07-20-buglogin-rebuild-discovery-design.md`

## Contract

IDs are permanent. Rename labels, never IDs. Removed capabilities remain tombstones with `drop`. Dependencies reference IDs here only.

- `keep`: retain proven upstream behavior; harden/rebrand narrowly.
- `replace`: preserve user job; replace legacy implementation or authority boundary.
- `later`: excluded until named gates pass.
- `drop`: intentionally unsupported; migration reports it only.
- Permission values: `local-owner`, `account`, `workspace-member`, `workspace-operator`, `workspace-admin`, `platform-admin`, `service`, `none`.
- Remote mutations require server ACL, idempotency, audit, stable errors. UI visibility never grants access.
- Secrets require explicit secret-read capability. API, MCP, logs, diagnostics, migration reports redact by default.

## Registry

| ID | Area / capability | Decision | Dependencies | Permission authority | API / MCP / UI / test / migration impact |
|---|---|---|---|---|---|
| `CAP-DESK-001` | Desktop / Tauri shell; offline bootstrap | keep | `CAP-REL-001` | `local-owner` | API: Health/version; MCP: none; UI: Shell/offline/recovery; Test: Windows/macOS/offline smoke; Migration: Supported settings only |
| `CAP-DESK-002` | Desktop / Typed command/event bridge | replace | `CAP-API-001`, `CAP-SEC-001` | Rust commands | API: Shared errors/events; MCP: Same commands; UI: Progress/cancel/errors; Test: Contract parity; Migration: Map supported legacy commands |
| `CAP-DESK-003` | Desktop / Default browser; deep links | keep | `CAP-PROF-002`, `CAP-PLAT-001` | `local-owner` | API: Optional local route; MCP: none; UI: Chooser/routing; Test: OS registration/routing; Migration: Preference only |
| `CAP-DESK-004` | Desktop / Tray, quit, restart, cleanup | keep | `CAP-RUN-003`, `CAP-REL-003` | `local-owner` | API: Local lifecycle; MCP: none; UI: Running/quit states; Test: Leak/crash recovery; Migration: Preserve data during upgrade |
| `CAP-WEB-001` | Web / Public/download/docs/status/legal | replace | `CAP-REL-002` | public; publish `platform-admin` | API: Read feeds; MCP: none; UI: Public responsive shell; Test: Links/SEO/a11y/download; Migration: No legacy UI |
| `CAP-WEB-002` | Web / Identity/account console | replace | `CAP-ID-001`, `CAP-ID-002`, `CAP-API-002` | `account` | API: required; MCP: Token tools prohibited; UI: Auth/security/devices; Test: Session/CSRF/revoke; Migration: Verified identities only |
| `CAP-WEB-003` | Web / Workspace operations | replace | `CAP-WS-001`, `CAP-TEAM-001`, `CAP-MEM-001`, `CAP-RES-001` | Server ACL | API: required; MCP: Scoped commands; UI: Dense operations UI; Test: Roles/a11y/failures; Migration: Data/grants, not pages |
| `CAP-WEB-004` | Web / Platform admin console | replace | `CAP-ADM-001`, `CAP-AUD-001`, `CAP-REL-002` | `platform-admin` | API: Separate scope; MCP: platform-admin only, token scopes still apply; UI: Elevated shell; Test: Isolation/audit; Migration: Approved governance only |
| `CAP-ADM-001` | Admin / Users/orgs/workspaces/incidents/jobs | replace | `CAP-ID-001`, `CAP-ORG-001`, `CAP-WS-001`, `CAP-AUD-001` | `platform-admin` | API: required; MCP: platform-admin only, token scopes still apply; UI: Admin required; Test: Cross-tenant denial; Migration: Deterministic mappings |
| `CAP-HR-001` | HR / Employment/person lifecycle | later | `CAP-ORG-001`, `CAP-MEM-001`, legal/retention approval | Dedicated HR role | API: Separate namespace; MCP: none; UI: Restricted UI; Test: Field ACL/retention; Migration: After legal/schema approval |
| `CAP-ORG-001` | Control / Organization tenancy | replace | `CAP-ID-001`, `CAP-SEC-001` | Org admin; audited override | API: required; MCP: scoped grants/roles; platform-admin override; token scopes still apply; UI: Switcher/admin; Test: Tenant isolation; Migration: Deterministic IDs |
| `CAP-WS-001` | Control / Workspace lifecycle/policy/quota | replace | `CAP-ORG-001`, `CAP-AUD-001` | `workspace-admin` | API: required; MCP: scoped grants/roles; platform-admin override; token scopes still apply; UI: Settings; Test: ACL/quota/offline; Migration: Map policies/exceptions |
| `CAP-TEAM-001` | Control / Team hierarchy/assignment | replace | `CAP-WS-001`, `CAP-MEM-001` | operator/admin | API: required; MCP: Scoped list/assign; UI: Team UI; Test: Boundaries/cycles; Migration: Map IDs/memberships |
| `CAP-MEM-001` | Control / Membership/invite/suspend/roles | replace | `CAP-ID-001`, `CAP-WS-001`, `CAP-SEC-001` | `workspace-admin`; self accept | API: required; MCP: No invite secrets; UI: Member/invite UI; Test: Expiry/replay/escalation; Migration: Reconcile memberships/invites |
| `CAP-RES-001` | Control / Canonical resource registry | replace | `CAP-WS-001`, `CAP-SEC-001` | Resource-type ACL | API: required; MCP: Addressable tools; UI: Inventory; Test: Type/state/tenant; Migration: Map or report every resource |
| `CAP-SEC-001` | Security / Roles/capabilities/grants/deny | replace | `CAP-ID-001` | Rust authority | API: Every protected route; MCP: Every tool; UI: Effective permissions only; Test: Full matrix; Migration: Translate; reject ambiguity |
| `CAP-AUD-001` | Security / Immutable audit/support access | replace | `CAP-ID-001`, `CAP-SEC-001` | Service append; scoped read | API: Query/export; MCP: All tools audited; UI: Audit UI; Test: Completeness/redaction; Migration: Preserve trusted actors/times |
| `CAP-ID-001` | Identity / Authentication; outbound password-recovery delivery later (unsupported this release) | replace | `CAP-SEC-001` | `account`; audited recovery | API: Authentication required; recovery requests unavailable until delivery ships; MCP: none; UI: Auth/account with explicit unavailable state; Test: credential/session abuse and fail-closed recovery; Migration: map verified identities; reset uncertain |
| `CAP-ID-002` | Identity / Sessions/devices/API/MCP tokens | replace | `CAP-ID-001`, `CAP-API-002` | `account`; admin service tokens | API: required; MCP: Token required; UI: Security UI; Test: Rotation/revoke/scope; Migration: Issue replacements; no plaintext import |
| `CAP-PROF-001` | Profiles / Metadata CRUD/clone/archive/restore | keep | `CAP-STOR-001`, `CAP-RES-001` | local; shared ACL | API: required; MCP: Scoped CRUD; UI: Desktop; web metadata; Test: Atomicity/roles/recovery; Migration: Direct supported mapping |
| `CAP-PROF-002` | Profiles / Launch/stop/status reconciliation | keep | `CAP-PROF-001`, `CAP-RUN-001`, `CAP-NET-001` | Local execution; shared lease | API: Local required; MCP: Scoped local lifecycle; UI: Desktop; Test: Crash/PID/concurrency; Migration: Launch-verify cohorts |
| `CAP-PROF-003` | Profiles / Groups/tags/notes/filter/bulk | keep | `CAP-PROF-001` | All-target authorization | API: required; MCP: Scoped bulk; UI: Desktop/web; Test: Partial failure/ACL; Migration: Deterministic mapping |
| `CAP-RUN-001` | Runtimes / Catalog/compatibility policy | replace | `CAP-REL-002`, `CAP-STOR-002` | publish admin; local select | API: Catalog; MCP: Read-only; UI: Settings/status; Test: Version matrix; Migration: Classify runtime |
| `CAP-RUN-002` | Runtimes / Download/verify/rollback | keep | `CAP-RUN-001`, `CAP-REL-001` | local; signed catalog | API: Progress/status; MCP: none; UI: Recovery; Test: Signature/interruption; Migration: Reuse verified binaries only |
| `CAP-RUN-003` | Runtimes / Adapter/process tree/crash cleanup | replace | `CAP-RUN-001`, `CAP-PROF-002`, `CAP-PLAT-001` | Rust local | API: Lifecycle; MCP: Scoped lifecycle; UI: Status/errors; Test: OS process integration; Migration: Per-profile adapter report |
| `CAP-RUN-004` | Runtimes / Legacy multi-runtime support | later | `CAP-RUN-003`, measured need, signed binaries | admin policy/local select | API: Versioned metadata; MCP: none initially; UI: After matrix approval; Test: Per-engine matrix; Migration: direct/adapter/partial/unsupported |
| `CAP-NET-001` | Proxies / Model/parser/assignment/bypass | keep | `CAP-STOR-001`, `CAP-SEC-002` | local; workspace grants | API: required/redacted; MCP: No secret-read default; UI: Desktop fast path/web manager; Test: Parser/ACL/leaks; Migration: Normalize/dedupe |
| `CAP-NET-002` | Proxies / Health/benchmark/batch checks | keep | `CAP-NET-001`, `CAP-JOB-001` | Proxy-use scope | Desktop: local check through OS-vault credential; REST/MCP batch jobs require connected-device dispatch; UI: Job states; Test: Slow/invalid/cancel/bounds; Migration: Optional verification |
| `CAP-NET-003` | Network / WireGuard lifecycle | later | `CAP-PROF-002`, `CAP-PLAT-001`, proven need | local + OS consent | API: Local initially; MCP: none; UI: Desktop; Test: Privilege/exclusion/cleanup; Migration: Report unsupported until enabled |
| `CAP-NET-004` | Network / DNS block/allow lists | later | `CAP-PROF-001`, reliability evidence | local; workspace policy | API: conditional; MCP: none; UI: Profile setting; Test: DNS failure/allowlist; Migration: Recognized settings only |
| `CAP-SEC-002` | Secrets / Storage/encryption/redaction | replace | `CAP-PLAT-001`, `CAP-ID-002` | Service vault; secret-read scope | API: Write-only default; MCP: Never return initially; UI: Re-auth reveal/copy; Test: Leak tests; Migration: Re-encrypt; no plaintext reports |
| `CAP-COOK-001` | Cookies / Import/export/read/delete | keep | `CAP-PROF-001`, `CAP-SEC-002` | Profile ACL + cookie scope | API: Local required; MCP: Import only initially; UI: Desktop; Test: Persistence/redaction/denial; Migration: Copy/verify; redact |
| `CAP-EXT-001` | Extensions / Package/source policy | replace | `CAP-SEC-002`, `CAP-REL-002` | Platform policy | API: Catalog/policy; MCP: Read-only; UI: Desktop/admin policy; Test: Hash/source/signature; Migration: Quarantine unknown |
| `CAP-EXT-002` | Extensions / Lifecycle/assignment | keep | `CAP-EXT-001`, `CAP-PROF-001`, `CAP-JOB-001` | Profile/resource ACL | API: required; MCP: Scoped install/assign; UI: Desktop/web metadata; Test: Rollback/assignment; Migration: Copy compatible; verify load |
| `CAP-SYNC-001` | Sync / Sync manifests, versions, conflicts | keep | `CAP-STOR-001`, `CAP-SEC-002` | resource ACL | API/MCP: required / scoped status; UI: sync center; Test/Migration: offline+conflict+retry / versioned package |
| `CAP-SYNC-002` | Sync / E2E encrypted chunks and key recovery | keep if sync ships | `CAP-SYNC-001`, `CAP-STOR-002`, `CAP-SEC-002` | client-held key | API/MCP: ciphertext / no key tools; UI: recovery UI; Test/Migration: wrong-key+corruption / re-encrypt after decrypt |
| `CAP-STOR-001` | Storage / Local metadata and atomic profile layout | replace | `CAP-MIG-001` schema | Rust local | API/MCP: domain APIs / indirect; UI: no direct UI; Test/Migration: crash/path/symlink / inspect-snapshot-apply |
| `CAP-STOR-002` | Storage / Object/blob adapter, checksums, retention, backup | later hosted; local backup keep | `CAP-STOR-001`, `CAP-SEC-002` | service + workspace policy | API/MCP: required hosted / none; UI: backup status; Test/Migration: hash/upload/restore / immutable copy |
| `CAP-JOB-001` | Operations / Long jobs, progress, cancel, retry | replace | `CAP-AUD-001` | caller action scope | API/MCP: required / required; UI: job center; Test/Migration: idempotency/terminal states / item results |
| `CAP-API-001` | API / Canonical command registry and errors | replace | `CAP-DOM-001`, `CAP-SEC-001`, `CAP-AUD-001` | domain authority | API/MCP: foundation / foundation; UI: generated handling; Test/Migration: registry/parity / legacy mapping |
| `CAP-API-002` | API / REST/OpenAPI, auth, binding | keep/replace boundary | `CAP-API-001`, `CAP-ID-002` | bearer/device/session + ACL | API/MCP: required / adapter; UI: generated clients; Test/Migration: OpenAPI/auth/rate / version compatibility |
| `CAP-MCP-001` | MCP / Opt-in scoped MCP tools and audit | conditional keep | `CAP-API-001`, `CAP-ID-002`, `CAP-AUD-001` | disabled default; scoped token | API/MCP: same commands / required; UI: visible status; Test/Migration: schema/ACL/redaction / no token migration |
| `CAP-MIG-001` | Migration / Read-only inspect, snapshot, package | replace | `CAP-STOR-001` | local owner; operator managed | API/MCP: report/status / none; UI: wizard/report; Test/Migration: deterministic fixtures / foundation |
| `CAP-MIG-002` | Migration / Transform, verify, cutover, rollback | replace | `CAP-MIG-001`, migrated domains | explicit operator approval | API/MCP: progress/report / none; UI: guided cutover; Test/Migration: counts/hashes/ACL/rollback / stop on mismatch |
| `CAP-PLAT-001` | Platform / Windows/macOS integration and secure storage | replace | `CAP-REL-001` | OS user + explicit elevation | API/MCP: diagnostics / none; UI: permission guidance; Test/Migration: physical/VM smoke / rebind paths |
| `CAP-REL-001` | Release / CI, signing, provenance, SBOM, verification | replace | `CAP-PLAT-001` | protected release automation | API/MCP: metadata / none; UI: optional provenance; Test/Migration: reproducibility/signature / no unsigned promotion |
| `CAP-REL-002` | Release / App/runtime/extension catalogs | replace | `CAP-REL-001`, `CAP-RUN-001`, `CAP-EXT-001` | platform-admin dual control | API/MCP: read/publish / read-only; UI: catalog UI; Test/Migration: bad catalog/rollback / signed versions only |
| `CAP-REL-003` | Release / Updater, restart safety, rollback, diagnostics | keep | `CAP-REL-001`, `CAP-DESK-004`, `CAP-DIAG-001` | local-owner | API/MCP: update status / none; UI: recovery UI; Test/Migration: interruption/data preservation / snapshot first |
| `CAP-DIAG-001` | Diagnostics / Redacted health, logs, support export, recovery | keep | `CAP-SEC-002`, `CAP-AUD-001` | local; time-limited support | API/MCP: health/export / scoped health; UI: health surfaces; Test/Migration: leak/actionability / evidence no secrets |
| `CAP-DOM-001` | Architecture / Stable IDs, entities, state machines, versions | replace | none | Rust domain | API/MCP: shared schemas / shared schemas; UI: generated types; Test/Migration: serialization/transitions / mapping keys |
| `CAP-DROP-001` | Excluded / Billing/Stripe/coupons/invoices in browser core | drop | none | none | API/MCP: none / none; UI: none; Test/Migration: assert no dependency / records only separate module |
| `CAP-DROP-002` | Excluded / TikTok/Etsy automation, cookie farming | drop | none | none | API/MCP: none / none; UI: none; Test/Migration: assert no routes/tools / unsupported report |
| `CAP-DROP-003` | Excluded / MITM/custom CA and broad tunnel matrix | drop | none | none | API/MCP: none / none; UI: none; Test/Migration: assert no trusted-root install / never migrate keys |

## Gates

| Gate | Exit condition |
|---|---|
| `G0-CONTRACT` | IDs/entities, permissions, commands, runtime adapter, storage/sync/migration formats, UI foundations, E2E IDs approved. |
| `G1-FOUNDATION` | Workspace builds; schemas generate; fixtures/CI exist; no duplicate authority models. |
| `G2-VERTICAL` | Account → workspace → profile → proxy parse → launch → stop → MCP → audit passes with real data. |
| `G3-BREADTH` | Domain suites pass; every migration fixture classified; no production mocks. |
| `G4-E2E` | Roles, offline, interruption, Windows/macOS, sync, rollback, a11y, visual gates pass. |
| `G5-RELEASE` | Security, signing, updater rollback, migration rehearsal, runbooks, residual-risk approval complete. |

## Change Control

Every change names changed IDs, compatibility effect, affected packets, migration effect, tests, and approving integration owner. New IDs use the next unused domain sequence; IDs never recycle.

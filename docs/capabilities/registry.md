# BugLogin Capability Registry

- Registry version: `1.0.0`
- Status: architecture contract; implementation not started
- Updated: `2026-07-20`
- Authority: capability IDs, disposition, dependencies, access, cross-surface impact
- Source: `AGENTS.md`, `docs/architecture/`, and current capability designs in `docs/superpowers/specs/`

## Contract

IDs are permanent. Rename labels, never IDs. Removed capabilities remain tombstones with `drop`. Dependencies reference IDs here only.

- `keep`: retain proven upstream behavior; harden/rebrand narrowly.
- `replace`: preserve user job; replace legacy implementation or authority boundary.
- `later`: excluded until named gates pass.
- `drop`: intentionally unsupported.
- Permission values: `local-owner`, `account`, `workspace-member`, `workspace-operator`, `workspace-admin`, `platform-admin`, `service`, `none`.
- Remote mutations require server ACL, idempotency, audit, stable errors. UI visibility never grants access.
- Secrets require explicit secret-read capability. API, MCP, logs, and diagnostics redact by default.

## Registry

| ID | Area / capability | Decision | Dependencies | Permission authority | API / MCP / UI / test impact |
|---|---|---|---|---|---|
| `CAP-DESK-001` | Desktop / Tauri shell; offline bootstrap | keep | `CAP-REL-001` | `local-owner` | API: Health/version; MCP: none; UI: Shell/offline/recovery; Test: Windows/macOS/offline smoke|
| `CAP-DESK-002` | Desktop / Typed command/event bridge | replace | `CAP-API-001`, `CAP-SEC-001` | Rust commands | API: Shared errors/events; MCP: Same commands; UI: Progress/cancel/errors; Test: Contract parity|
| `CAP-DESK-003` | Desktop / Default browser; deep links | keep | `CAP-PROF-002`, `CAP-PLAT-001` | `local-owner` | API: Optional local route; MCP: none; UI: Chooser/routing; Test: OS registration/routing|
| `CAP-DESK-004` | Desktop / Tray, quit, restart, cleanup | keep | `CAP-RUN-003`, `CAP-REL-003` | `local-owner` | API: Local lifecycle; MCP: none; UI: Running/quit states; Test: Leak/crash recovery|
| `CAP-WEB-001` | Web / Public/download/docs/status/legal | replace | `CAP-REL-002` | public; publish `platform-admin` | API: Read feeds; MCP: none; UI: Public responsive shell; Test: Links/SEO/a11y/download|
| `CAP-WEB-002` | Web / Identity/account console | replace | `CAP-ID-001`, `CAP-ID-002`, `CAP-API-002` | `account` | API: required; MCP: Token tools prohibited; UI: Auth/security/devices; Test: Session/CSRF/revoke|
| `CAP-WEB-003` | Web / Workspace operations | replace | `CAP-WS-001`, `CAP-TEAM-001`, `CAP-MEM-001`, `CAP-RES-001` | Server ACL | API: required; MCP: Scoped commands; UI: Dense operations UI; Test: Roles/a11y/failures|
| `CAP-WEB-004` | Web / Platform admin console | replace | `CAP-ADM-001`, `CAP-AUD-001`, `CAP-REL-002` | `platform-admin` | API: Platform Admin account role; MCP: Platform Admin role; UI: Elevated shell; Test: Isolation/audit|
| `CAP-ADM-001` | Admin / Users/orgs/workspaces/incidents/jobs | replace | `CAP-ID-001`, `CAP-ORG-001`, `CAP-WS-001`, `CAP-MEM-001`, `CAP-AUD-001` | `platform-admin` | API/MCP: authenticated Platform Admin role grants full capability access regardless of API-token scopes; explicit DENY grants and account status still apply; UI: Admin required; Test: Cross-tenant denial|
| `CAP-HR-001` | HR / Employment/person lifecycle | later | `CAP-ORG-001`, `CAP-MEM-001`, legal/retention approval | Dedicated HR role | API: Separate namespace; MCP: none; UI: Restricted UI; Test: Field ACL/retention|
| `CAP-ORG-001` | Control / Organization tenancy | replace | `CAP-ID-001`, `CAP-SEC-001` | Org admin; audited override | API: required; MCP: scoped grants/roles; Platform Admin role overrides token scopes; explicit denies apply; UI: Switcher/admin; Test: Tenant isolation|
| `CAP-WS-001` | Control / Workspace lifecycle/policy/quota | replace | `CAP-ORG-001`, `CAP-AUD-001` | `workspace-admin` | API: required; MCP: scoped grants/roles; Platform Admin role overrides token scopes; explicit denies apply; UI: Settings; Test: ACL/quota/offline|
| `CAP-TEAM-001` | Control / Team hierarchy/assignment | replace | `CAP-WS-001`, `CAP-MEM-001` | operator/admin | API: required; MCP: Scoped list/assign; UI: Team UI; Test: Boundaries/cycles|
| `CAP-MEM-001` | Control / Membership/invite/suspend/roles | replace | `CAP-ID-001`, `CAP-WS-001`, `CAP-SEC-001` | `workspace-admin`; self accept | API: required; MCP: No invite secrets; UI: Member/invite UI; Test: Expiry/replay/escalation|
| `CAP-RES-001` | Control / Canonical resource registry | replace | `CAP-WS-001`, `CAP-SEC-001` | Resource-type ACL | API: required; MCP: Addressable tools; UI: Inventory; Test: Type/state/tenant|
| `CAP-SEC-001` | Security / Roles/capabilities/grants/deny | replace | `CAP-ID-001` | Rust authority | API: Every protected route; MCP: Every tool; UI: Effective permissions only; Test: Full matrix|
| `CAP-AUD-001` | Security / Immutable audit/support access | replace | `CAP-ID-001`, `CAP-SEC-001` | Service append; scoped read | API: Query/export; MCP: All tools audited; UI: Audit UI; Test: Completeness/redaction|
| `CAP-ID-001` | Identity / Password and optional BugLab ID SSO authentication; outbound password-recovery delivery unsupported | replace | `CAP-SEC-001` | `account`; provider subject linked to verified identity; audited recovery | API: Authentication required; BugLab SSO opt-in only after provider callback/state confirmation; MCP: none; UI: password and SSO login; Test: credential/session abuse, state/replay, role neutrality|
| `CAP-ID-002` | Identity / Sessions/devices/API/MCP tokens | replace | `CAP-ID-001`, `CAP-API-002` | `account`; admin service tokens | API: required; MCP: Token required; UI: Security UI; Test: Rotation/revoke/scope|
| `CAP-PROF-001` | Profiles / Metadata CRUD/clone/archive/restore | keep | `CAP-STOR-001`, `CAP-RES-001` | local; shared ACL | API: required, including authorized profile detail and one-time profile-scoped web viewer handoff; MCP: Scoped CRUD; UI: Desktop; web metadata/detail, with current connection observed separately from the current site's Cloudflare trace; Test: Atomicity/roles/recovery and read ACL|
| `CAP-PROF-002` | Profiles / Launch/stop/status reconciliation | keep | `CAP-PROF-001`, `CAP-RUN-001`, `CAP-NET-001` | Local execution; shared lease | API: Local required; MCP: Scoped local lifecycle; UI: Desktop; Test: Crash/PID/concurrency|
| `CAP-PROF-003` | Profiles / Groups/tags/notes/filter/bulk | keep | `CAP-PROF-001` | All-target authorization | API: required; MCP: Scoped bulk; UI: Desktop/web; Test: Partial failure/ACL|
| `CAP-RUN-001` | Runtimes / Catalog/compatibility policy | replace | `CAP-REL-002`, `CAP-STOR-002` | publish admin; local select | API: Catalog; MCP: Read-only; UI: Settings/status; Test: Version matrix|
| `CAP-RUN-002` | Runtimes / Download/verify/rollback | keep | `CAP-RUN-001`, `CAP-REL-001` | local; signed catalog | API: Progress/status; MCP: none; UI: Recovery; Test: Signature/interruption|
| `CAP-RUN-003` | Runtimes / Adapter/process tree/crash cleanup | replace | `CAP-RUN-001`, `CAP-PROF-002`, `CAP-PLAT-001` | Rust local | API: Lifecycle; MCP: Scoped lifecycle; UI: Status/errors; Test: OS process integration|
| `CAP-RUN-004` | Runtimes / Legacy multi-runtime support | later | `CAP-RUN-003`, measured need, signed binaries | admin policy/local select | API: Versioned metadata; MCP: none initially; UI: After matrix approval; Test: Per-engine matrix|
| `CAP-NET-001` | Proxies / Model/parser/assignment/bypass | keep | `CAP-STOR-001`, `CAP-SEC-002` | local; workspace grants | API: required/redacted; MCP: No secret-read default; UI: Desktop fast path/web manager; Test: Parser/ACL/leaks|
| `CAP-NET-002` | Proxies / Health/benchmark/batch checks | keep | `CAP-NET-001`, `CAP-JOB-001` | Proxy-use scope | Desktop: local check through OS-vault credential; REST/MCP batch jobs require connected-device dispatch; UI: Job states; Test: Slow/invalid/cancel/bounds|
| `CAP-NET-003` | Network / WireGuard lifecycle | later | `CAP-PROF-002`, `CAP-PLAT-001`, proven need | local + OS consent | API: Local initially; MCP: none; UI: Desktop; Test: Privilege/exclusion/cleanup|
| `CAP-NET-004` | Network / DNS block/allow lists | later | `CAP-PROF-001`, reliability evidence | local; workspace policy | API: conditional; MCP: none; UI: Profile setting; Test: DNS failure/allowlist|
| `CAP-SEC-002` | Secrets / Storage/encryption/redaction | replace | `CAP-PLAT-001`, `CAP-ID-002` | Service vault; secret-read scope | API: Write-only default; MCP: Never return initially; UI: Re-auth reveal/copy; Test: Leak tests|
| `CAP-COOK-001` | Cookies / Import/export/read/delete | keep | `CAP-PROF-001`, `CAP-SEC-002` | Profile ACL + cookie scope | API: Local required; MCP: Import only initially; UI: Desktop; Test: Persistence/redaction/denial|
| `CAP-EXT-001` | Extensions / Package/source policy | replace | `CAP-SEC-002`, `CAP-REL-002` | Platform policy | API: Catalog/policy; MCP: Read-only; UI: Desktop/admin policy; Test: Hash/source/signature|
| `CAP-EXT-002` | Extensions / Lifecycle/assignment | keep | `CAP-EXT-001`, `CAP-PROF-001`, `CAP-JOB-001` | Profile/resource ACL | API: required; MCP: Scoped install/assign; UI: Desktop/web metadata; Test: Rollback/assignment|
| `CAP-SYNC-001` | Sync / Sync manifests, versions, conflicts | keep | `CAP-STOR-001`, `CAP-SEC-002` | resource ACL | API/MCP: required / scoped status; UI: sync center; Test: offline+conflict+retry|
| `CAP-SYNC-002` | Sync / E2E encrypted chunks and key recovery | keep if sync ships | `CAP-SYNC-001`, `CAP-STOR-002`, `CAP-SEC-002` | client-held key | API/MCP: ciphertext / no key tools; UI: recovery UI; Test: wrong-key+corruption|
| `CAP-STOR-001` | Storage / Local metadata and atomic profile layout | replace | `CAP-DOM-001` | Rust local | API/MCP: domain APIs / indirect; UI: no direct UI; Test: crash/path/symlink |
| `CAP-STOR-002` | Storage / Object/blob adapter, checksums, retention, backup | later hosted; local backup keep | `CAP-STOR-001`, `CAP-SEC-002` | service + workspace policy | API/MCP: required hosted / none; UI: backup status; Test: hash/upload/restore|
| `CAP-JOB-001` | Operations / Long jobs, progress, cancel, retry | replace | `CAP-AUD-001` | caller action scope | API/MCP: required / required; UI: job center; Test: idempotency/terminal states|
| `CAP-API-001` | API / Canonical command registry and errors | replace | `CAP-DOM-001`, `CAP-SEC-001`, `CAP-AUD-001` | domain authority | API/MCP: foundation / foundation; UI: generated handling; Test: registry/parity|
| `CAP-API-002` | API / REST/OpenAPI, auth, binding | keep/replace boundary | `CAP-API-001`, `CAP-ID-002` | bearer/device/session + ACL | API/MCP: required / adapter; UI: generated clients; Test: OpenAPI/auth/rate|
| `CAP-MCP-001` | MCP / Opt-in scoped MCP tools and audit | conditional keep | `CAP-API-001`, `CAP-ID-002`, `CAP-AUD-001` | disabled default; scoped token | API/MCP: same commands / required; UI: visible status; Test: schema/ACL/redaction|
| `CAP-PLAT-001` | Platform / Windows/macOS integration and secure storage | replace | `CAP-REL-001` | OS user + explicit elevation | API/MCP: diagnostics / none; UI: permission guidance; Test: physical/VM smoke|
| `CAP-REL-001` | Release / CI, signing, provenance, SBOM, verification | replace | `CAP-PLAT-001` | protected release automation | API/MCP: metadata / none; UI: optional provenance; Test: reproducibility/signature|
| `CAP-REL-002` | Release / App/runtime/extension catalogs | replace | `CAP-REL-001`, `CAP-RUN-001`, `CAP-EXT-001` | platform-admin dual control | API/MCP: read/publish / read-only; UI: catalog UI; Test: bad catalog/rollback|
| `CAP-REL-003` | Release / Updater, restart safety, rollback, diagnostics | keep | `CAP-REL-001`, `CAP-DESK-004`, `CAP-DIAG-001` | local-owner | API/MCP: update status / none; UI: recovery UI; Test: interruption/data preservation|
| `CAP-DIAG-001` | Diagnostics / Redacted health, logs, support export, recovery | keep | `CAP-SEC-002`, `CAP-AUD-001` | local; time-limited support | API/MCP: health/export / scoped health; UI: health surfaces; Test: leak/actionability|
| `CAP-DOM-001` | Architecture / Stable IDs, entities, state machines, versions | replace | none | Rust domain | API/MCP: shared schemas / shared schemas; UI: generated types; Test: serialization/transitions|
| `CAP-DROP-001` | Excluded / Billing/Stripe/coupons/invoices in browser core | drop | none | none | API/MCP: none / none; UI: none; Test: assert no dependency|
| `CAP-DROP-002` | Excluded / TikTok/Etsy automation, cookie farming | drop | none | none | API/MCP: none / none; UI: none; Test: assert no routes/tools|
| `CAP-DROP-003` | Excluded / MITM/custom CA and broad tunnel matrix | drop | none | none | API/MCP: none / none; UI: none; Test: assert no trusted-root install|

## Gates

| Gate | Exit condition |
|---|---|
| `G0-CONTRACT` | IDs/entities, permissions, commands, runtime adapter, storage and sync formats, UI foundations, E2E IDs approved. |
| `G1-FOUNDATION` | Workspace builds; schemas generate; fixtures/CI exist; no duplicate authority models. |
| `G2-VERTICAL` | Account → workspace → profile → proxy parse → launch → stop → MCP → audit passes with real data. |
| `G3-BREADTH` | Domain suites pass; every sync fixture classified; no production mocks. |
| `G4-E2E` | Roles, offline, interruption, Windows/macOS, sync, rollback, a11y, visual gates pass. |
| `G5-RELEASE` | Security, signing, updater rollback, release rehearsal, runbooks, residual-risk approval complete. |

## Change Control

Every change names changed IDs, compatibility effect, affected packets, data-model effect, tests, and approving integration owner. New IDs use the next unused domain sequence; IDs never recycle.

# Windows and macOS E2E Acceptance Pack

- Status: release-gate specification
- Execution: signed release-candidate artifacts only
- Evidence: result JSON plus redacted logs/screenshots per case

## Candidate support matrix

Product/release owners must freeze exact supported OS versions before RC1. Until then test this broad matrix.

| Family | Coverage | Architecture | Notes |
|---|---|---|---|
| Windows 11 | Current supported releases | x86_64 | Installer, WebView2, deep links, default browser |
| Windows 10 | Final compatibility lane | x86_64 | Support decision required before GA |
| macOS | Current plus two previous supported majors | arm64 | Signed, notarized, Gatekeeper clean |
| macOS | Oldest supported Intel release | x86_64 | Native Intel testing; Rosetta is insufficient |

Physical or dedicated hosted instances required. Cross-compilation is insufficient.

## Environments

| Name | Network | Data | Purpose |
|---|---|---|---|
| `E2E-CLEAN` | Normal | Empty root | Install/onboarding |
| `E2E-OFFLINE` | None | Existing local profiles | Offline behavior |
| `E2E-DEGRADED` | Latency/loss/DNS failure | Sync/API | Retry/recovery |
| `E2E-PROXY` | Controlled HTTP/SOCKS | Synthetic auth | Proxy/redaction |
| `E2E-UPGRADE` | Normal | Previous stable data | Upgrade/rollback |
| `E2E-MIGRATION` | Isolated | Migration fixtures | Migration |
| `E2E-ACL` | Normal | Multi-user fixtures | Server authority |

Each case records case ID, artifact SHA-256, versions, OS build, architecture, fixture, UTC times, steps, assertions, process list, logs, screenshots, result, defect, tester, reviewer.

## Acceptance cases

### Install, identity, update

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-INST-001` | Clean install | Signature valid; expected paths; launch |
| `E2E-INST-002` | Standard-user use | No undocumented admin requirement |
| `E2E-INST-003` | Uninstall keep-data | Choice honored; app-owned files only |
| `E2E-INST-004` | Uninstall delete-data | Explicit confirmation; scoped deletion |
| `E2E-INST-005` | Deep-link registration | Approved schemes route deterministically |
| `E2E-INST-006` | Default-browser registration | OS-visible, reversible, chooser works |
| `E2E-UPD-001` | Stable to RC | Profiles/settings preserved |
| `E2E-UPD-002` | Interrupted update | Old/new version remains launchable |
| `E2E-UPD-003` | Bad checksum/signature | Rejected before execution |
| `E2E-UPD-004` | App rollback | Executable/data compatibility preserved |

### Profile lifecycle

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-PRO-001` | Create Wayfern profile | Valid metadata/fingerprint; isolated path |
| `E2E-PRO-002` | Duplicate/invalid input | Clear error; no partial profile |
| `E2E-PRO-003` | Launch/browse/stop/relaunch | State reconciles; storage persists |
| `E2E-PRO-004` | Concurrent launches | Isolation; no process/port collision |
| `E2E-PRO-005` | Double-launch same profile | One authoritative launch; clear status |
| `E2E-PRO-006` | Force-kill browser/app | Orphans cleaned before next launch |
| `E2E-PRO-007` | Clone profile | New identity/path; selected data only |
| `E2E-PRO-008` | Soft delete/restore | Retention and references correct |
| `E2E-PRO-009` | Permanent delete | Explicit; no collateral deletion |
| `E2E-PRO-010` | Ephemeral profile | Data removed on close per contract |
| `E2E-PRO-011` | Unicode/long names | Vietnamese/Unicode safe |
| `E2E-PRO-012` | Low disk/locked path | No corruption; recovery guidance |

### Proxy, VPN, DNS

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-NET-001` | HTTP/HTTPS/SOCKS4/SOCKS5 | Egress matches endpoint |
| `E2E-NET-002` | Auth proxy | Works; credentials absent from evidence |
| `E2E-NET-003` | Dead/invalid proxy | Explicit policy; retry/cancel |
| `E2E-NET-004` | Dynamic proxy URL | Refresh deterministic/audited |
| `E2E-NET-005` | IPv6/bypass rules | Parser/routing correct |
| `E2E-NET-006` | WireGuard connect/disconnect | Route scoped; cleanup complete |
| `E2E-NET-007` | Proxy + VPN assignment | Rejected at UI and backend |
| `E2E-NET-008` | DNS blocklist | Block/allowlist recovery correct |
| `E2E-NET-009` | Sleep/wake/network switch | Reconnect or actionable failure |

### Cookies, extensions, import/export

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-DATA-001` | Cookie import/export | Authorized flow; no logs |
| `E2E-DATA-002` | Cookie copy | Selected targets only; isolation |
| `E2E-DATA-003` | Extension install/assign/update/remove | Source/checksum policy; persistence |
| `E2E-DATA-004` | Invalid extension | Rejected safely; profile remains usable |
| `E2E-DATA-005` | Browser import dry-run | Counts/conflicts before copy |
| `E2E-DATA-006` | Import cancellation | Source unchanged; staging cleaned |
| `E2E-DATA-007` | Neutral export/re-import | Hashes/IDs match |

### UX, API, MCP, security

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-UX-001` | Groups/tags/notes/filter | Persistent deterministic state |
| `E2E-UX-002` | Bulk actions | Per-item results; partial failures visible |
| `E2E-UX-003` | Loading/empty/error/offline/partial/locked | Actionable; no silent actions |
| `E2E-UX-004` | Keyboard-only workflow | Focus/dialog/table operable |
| `E2E-UX-005` | 100/125/150/200% scaling | No critical clipping/overlap |
| `E2E-UX-006` | Minimum window | Essential actions reachable |
| `E2E-UX-007` | English/Vietnamese | No missing keys/destructive truncation |
| `E2E-API-001` | API default | Disabled; port closed |
| `E2E-API-002` | API auth | Localhost; bearer required |
| `E2E-API-003` | API profile lifecycle | Same validation/state as UI |
| `E2E-API-004` | OpenAPI conformance | Docs/handlers/errors match |
| `E2E-MCP-001` | MCP default | Disabled; no tools exposed |
| `E2E-MCP-002` | Scoped MCP tool | Permission/audit/result match |
| `E2E-MCP-003` | Revoked/expired token | Immediate rejection |
| `E2E-SEC-001` | Malicious archive/path | No traversal/overwrite/execute |
| `E2E-SEC-002` | Diagnostics export | Secrets/cookies/tokens redacted |

### Sync, workspace, ACL

Run only when capability ships.

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-SYNC-001` | Initial encrypted sync | Counts/hashes match |
| `E2E-SYNC-002` | Offline edit/reconnect | Defined merge; no silent loss |
| `E2E-SYNC-003` | Interrupted transfer | Verified resume; no duplicate object |
| `E2E-SYNC-004` | Checksum mismatch | Reject and recover |
| `E2E-SYNC-005` | Concurrent launch | Lease/lock prevents corruption |
| `E2E-SYNC-006` | Revoke/lease expiry | Access removed server/client |
| `E2E-SYNC-007` | Key/password loss | Explicit recovery semantics |
| `E2E-ACL-001` | Role/resource matrix | Effective access exact |
| `E2E-ACL-002` | Frontend bypass | Backend rejects mutation |

### Migration and recovery

| ID | Journey | Pass criteria |
|---|---|---|
| `E2E-MIG-001` | `MIG-002` full migration | Counts, hashes, launch, cookies, proxy, extension |
| `E2E-MIG-002` | Unsupported runtime | Data retained; launch blocked |
| `E2E-MIG-003` | Secret canary | Zero leakage |
| `E2E-MIG-004` | Interrupted migration | Source unchanged; safe resume/cleanup |
| `E2E-MIG-005` | Rollback/repeat | Pre-state restored; identical rerun |
| `E2E-REC-001` | Corrupt metadata | Isolated damage; recovery report |
| `E2E-REC-002` | Crash during write | Atomic state after restart |

## Platform checks

### Windows

- Authenticode chain and timestamp validate.
- NSIS install/upgrade/repair/uninstall pass.
- WebView2 present/missing/update scenarios pass.
- Defender/SmartScreen result recorded; no unexpected quarantine.
- Long paths, locked files, non-ASCII usernames, sleep/wake, taskbar identity, protocol/default-browser registration pass.
- Exit leaves no browser, proxy, VPN, daemon, updater, or lock process.

### macOS

- `codesign --verify --deep --strict`, notarization, stapling, Gatekeeper assessment pass.
- DMG mount, drag-install, upgrade, first launch, quarantine pass.
- Apple Silicon and Intel artifacts have expected architectures.
- Keychain prompts/permissions are minimal and explained.
- Spaces/full-screen, sleep/wake, non-ASCII home path, deep links/default browser pass.
- Exit leaves no browser, proxy, VPN, daemon, updater, or lock process.

## Release thresholds

- Required cases: 100% pass on every supported OS/architecture lane.
- Critical/high data, auth, ACL, runtime, sync, migration, update, or cleanup defects: zero.
- Flaky required case remains failed until root cause fixed.
- `not applicable` requires release manifest capability exclusion.
- QA lead and release commander sign final evidence index.

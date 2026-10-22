# Migration Source and Version Matrix

- Status: implementation contract
- Date: 2026-07-20
- Scope: copy-only migration into rebuilt BugLogin desktop product
- Evidence:
  - Legacy BugLogin `v1.1.84`, commit `81f9af637825c79c7ca60bbbdf2c11b4fecfa310`, 2026-07-08
  - DonutBrowser baseline `v0.34.0`, commit `f70a6c9671b17ab03a6e6f87a7362f8076c8339d`, 2026-10-16

## Support classification

| Class | Meaning | Import result |
|---|---|---|
| Direct | Destination equivalent exists | Imported without semantic loss |
| Adapter | Compatibility adapter required | Imported; limitation recorded |
| Partial | Safe subset only | Imported fields and omissions recorded |
| Unsupported | No safe destination behavior | Not imported; source reference retained |
| Unknown | Layout/version unproven | Inspect only; cutover blocked |

## Source matrix

| Source | Version/range | Evidence required | Default class | Eligibility |
|---|---|---|---|---|
| Legacy BugLogin | `1.1.84` | Three version files, source-root hash | Direct/adapter by entity | One direct source diff, signed clone, and post-import reconciliation; no repeated rehearsal cycle |
| Legacy BugLogin | `1.1.0`–`1.1.83` | Fixture per storage-layout epoch | Unknown until proven | No |
| Legacy BugLogin | `<1.1.0` | Exact build and directory inventory | Unknown | No |
| DonutBrowser | `0.34.0` | Export or immutable data-root snapshot | Direct where schema matches | Rehearsal only |
| DonutBrowser | Other versions | Exact tag/commit plus compatibility test | Unknown | No |
| Chromium-family import | Installed profile directory | Browser/version, Local State, profile inventory | Partial | User-driven only |
| Firefox-family import | Installed profile directory | Browser/version, `profiles.ini`, profile inventory | Partial | User-driven only |
| Neutral export | `buglogin.export/1` | Manifest, schema, hashes, signature policy | Direct | Yes |

Unsupported versions must never be guessed from timestamps or treated as the nearest known version.

## Source roots

| OS | Default legacy root | Override precedence |
|---|---|---|
| Windows | `%LOCALAPPDATA%\BugLogin` | `BUGLOGIN_DATA_DIR`, then `BUGLOGINBROWSER_DATA_DIR`, then default |
| macOS | `~/Library/Application Support/BugLogin` | `BUGLOGIN_DATA_DIR`, then `BUGLOGINBROWSER_DATA_DIR`, then default |
| Development | Platform-local `BugLoginDev` | Same overrides |

Inspector accepts an explicit source root. It must not scan unrelated user directories without confirmation.

## Storage inventory

| Domain | Expected location | Policy | Verification |
|---|---|---|---|
| Profile metadata | `profiles/<uuid>/metadata.json` | Direct/adapter | JSON, UUID, runtime, references |
| Browser data | `profiles/<uuid>/profile/**` | Copy object-by-object | File/byte counts, SHA-256 |
| Soft-deleted profiles | `profiles-deleted/**` | Excluded by default | Count reported; explicit restore only |
| Managed runtimes | `binaries/<runtime>/<version>/**` | Never trusted-copy | Version recorded; destination downloads |
| Proxies | `proxies/**` | Normalize then import | Protocol, host, port, auth presence |
| VPN | `vpn/**` | Partial, opt-in | Secret redaction, parse, mutual exclusion |
| Extensions | `extensions/**` | Partial/direct | Manifest, ID, source, checksum |
| Settings | `settings/**` | Allowlist only | Unknown keys reported; secrets excluded |
| General data | `data/**` | Entity-specific | No blind copy |
| Cache/logs | Cache and log roots | Never import | Presence/size only |
| Sync/control plane | Legacy database/object store | Separate export | Workspace, ACL, object, hash reconciliation |

## Entity/runtime matrix

| Source | Destination | Class | Rule |
|---|---|---|---|
| Profile identity/name/tags/note/group | Profile metadata | Direct | Deterministic mapped ID |
| Wayfern | Wayfern | Direct if contract accepts config | Regenerate only derived fields |
| Camoufox | Archived unsupported profile | Unsupported | Preserve data; never auto-convert |
| Bugox/Bugium/Bugmini/Cloak/Thorium/Helium | Adapter/unsupported result | Adapter or unsupported | Explicit per-runtime approval |
| Proxy assignment | Proxy plus profile reference | Direct | Normalize; never print credentials |
| VPN assignment | VPN plus profile reference | Partial | Supported WireGuard fields only |
| Cookies | Profile data or neutral cookies | Direct/partial | Compatible key material or controlled export |
| Extensions | Package/reference plus assignments | Partial | Block missing/invalid/untrusted packages |
| Groups | Group records | Direct | Stable ID; missing group becomes null + warning |
| Workspace/team/member | Control-plane entities | Direct after ACL approval | Server remains authority |
| Grants/shares/leases | ACL/grant entities | Adapter | Semantic differences require approval |
| Sync objects | Destination object store | Direct after hash check | Preserve content/source mapping |
| Billing/commerce | Audit-only export | Unsupported | Separate business migration |
| Automation credentials | Excluded | Unsupported | Never enter desktop package |

## Deterministic IDs

- Fixed UUID namespace: `f5d2d691-b715-48b1-bc16-2855ef99586b` (`buglogin.migration.v1`).
- Input: `source_system + ":" + source_instance_id + ":" + entity_type + ":" + source_id`.
- Output: UUIDv5.
- Collision with different provenance: hard stop.
- Re-run: identical IDs, no duplicates.
- Mapping: `reports/id-map.jsonl`, sorted by entity type then source ID.

## Destination compatibility

| State | Action |
|---|---|
| Reader supports `buglogin.export/1` | Import supported minor features |
| Unknown optional fields | Ignore only when marked optional; report |
| Unknown required capability | Stop |
| Newer major schema | Stop |
| Non-empty destination | Require merge mode and collision preflight |
| Destination below package minimum | Stop |

## Inspector output

`inspect` outputs source identity, detected version, roots, entity counts, bytes, runtime classes, secret scan, invalid records, dangling references, source tree hash, and cutover eligibility. It is read-only and creates no files in the source root.

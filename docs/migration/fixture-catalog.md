# Migration Fixture Catalog

- Fixture root: `fixtures/migration/<fixture-id>/`
- Committed evidence: 28/28 fixtures (`MIG-001`, `MIG-002`, `MIG-003`, `MIG-004`, `MIG-005`, `MIG-006`, `MIG-007`, `MIG-008`, `MIG-009`, `MIG-010`, `MIG-011`, `MIG-012`, `MIG-013`, `MIG-014`, `MIG-015`, `MIG-016`, `MIG-017`, `MIG-018`, `MIG-019`, `MIG-020`, `MIG-021`, `MIG-022`, `MIG-023`, `MIG-024`, `MIG-025`, `MIG-026`, `MIG-027`, `MIG-028`).
- Fixture coverage is complete; production migration still requires signed Windows/macOS rehearsal, rollback, and live hosted-adapter evidence.
- Contents: `source/`, `expected/inspect.json`, `expected/export-summary.json`, `expected/import-results.jsonl`, `README.md`
- Data: synthetic identities, domains, IPs, credentials, cookies, content
- Scale fixtures: deterministic generator plus fixed seed

## Required fixtures

| ID | Source shape | Assertion | Expected result |
|---|---|---|---|
| `MIG-001-empty-1.1.84` | Valid empty root | Empty source valid | Zero imports; eligible |
| `MIG-002-wayfern-complete` | Wayfern profile, group, proxy, extension | Vertical happy path | All direct; launch candidate |
| `MIG-003-cookie-persistence` | Synthetic cookie DB | Controlled cookie handling | Hash preserved or explicit conversion |
| `MIG-004-proxy-protocols` | HTTP/HTTPS/SOCKS4/SOCKS5/IPv6/auth | Normalize, redact | Valid imports; zero leakage |
| `MIG-005-proxy-invalid-mixed` | Valid and malformed proxy rows | Deterministic partial failure | Per-row result; policy controls commit |
| `MIG-006-vpn-wireguard` | Synthetic WireGuard config | Secret policy, mutual exclusion | Encrypted/secret-ref only |
| `MIG-007-camoufox-unsupported` | Camoufox profile data | Never auto-convert | Data preserved; launch blocked |
| `MIG-008-runtime-aliases` | Bugox/Bugium/Bugmini/Cloak/Thorium/Helium | Explicit classification | Approved adapter/unsupported result |
| `MIG-009-dangling-references` | Missing group/proxy/extension refs | Reference policy | Optional null+warning; required stops |
| `MIG-010-duplicate-source-ids` | Same ID, different content | Collision safety | Hard stop |
| `MIG-011-idempotent-repeat` | Same package twice | Idempotency | Second run zero creates/updates |
| `MIG-012-destination-collision` | Mapped ID occupied by other provenance | No overwrite | Hard stop |
| `MIG-013-soft-deleted` | Active and deleted profiles | Safe default | Deleted reported, not imported |
| `MIG-014-corrupt-metadata` | Truncated JSON/invalid UUID/types | Boundary validation | Error; no source mutation |
| `MIG-015-path-traversal` | Archive and symlink escape | Extraction safety | Reject before extraction |
| `MIG-016-hash-mismatch` | Modified object | Integrity | Hard stop |
| `MIG-017-secret-canary` | Canary in sensitive fields | Redaction | Absent from all evidence |
| `MIG-018-acl-complete` | Workspace/members/grants/share/lease | ACL equivalence | Exact effective access |
| `MIG-019-acl-unknown-role` | Unknown role/access mode | No downgrade | ACL import blocked |
| `MIG-020-sync-object-set` | Metadata plus object hashes | Reconciliation | Exact counts/hashes |
| `MIG-021-interrupted-copy` | Forced stop during staging | Resume safety | Source unchanged; safe resume/discard |
| `MIG-022-disk-full` | Destination quota exhausted | Data-loss prevention | No live commit |
| `MIG-023-case-unicode` | Case collisions, NFC/NFD, Vietnamese | Cross-OS consistency | Stable IDs/names |
| `MIG-024-long-paths` | Windows/macOS deep paths | Path handling | Success or explicit unsupported |
| `MIG-025-locked-files` | Source files locked | Immutable snapshot requirement | Cutover blocked |
| `MIG-026-unknown-version` | Unsupported root | No guessing | Inspect only |
| `MIG-027-encrypted-package` | Correct/incorrect password | Authenticated decryption | Success/fail with no plaintext residue |
| `MIG-028-large-generated` | 10k profiles, 100k objects | Scale/determinism | Stable counts/hashes; budget enforced |

## Golden report fields

Every expected result asserts source/destination IDs, classification/reason, result counts, source/destination hashes, reference/collision decisions, secret scan, source hash before/after, rerun delta, and cutover eligibility.

## Acceptance

- Windows/macOS produce identical logical reports after path normalization.
- Ordering is deterministic.
- Machine-specific absolute paths are absent or explicitly redacted.
- Sensitive file entries serialize `sha256: redacted`; exact integrity digests exist only in memory during a migration operation.
- Source hash is unchanged after inspect, export, failed/successful import, resume, and rollback.
- Fixture changes require source-intent and golden-output review; never regenerate goldens blindly.

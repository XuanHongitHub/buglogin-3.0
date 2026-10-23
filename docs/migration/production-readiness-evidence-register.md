# Migration Production-Readiness Evidence Register

Status: audit snapshot, 2026-10-13

This register separates evidence that can be produced from sanitized fixtures from evidence that requires a production clone, approved staging services, or a real cutover window. No production data, source root, credentials, or production claim is created by this document.

## Fastest offline artifacts

| Artifact | Current evidence | Fastest action | Release meaning |
|---|---|---|---|
| Synthetic package-to-rollback report | Present in `crates/migration/tests/rehearsal.rs` | Keep the evidence schema assertions green | Synthetic boundary only |
| ACL-complete synthetic rehearsal | Present for `MIG-018-acl-complete` | Re-run with the release-candidate commit and archive JSON output | Control-plane mapping confidence |
| Negative confirmation evidence | Present for cutover and rollback confirmation | Retain error codes in the rehearsal artifact | Prevents accidental writes |
| Repeat activation and rollback evidence | Present in synthetic rehearsal and rollback retry test | Add both outputs to the RC evidence bundle | Idempotency/rollback confidence |
| Fixture coverage index | Present: 28 fixtures | Run the full migration fixture suite and archive the command log | Sanitized regression breadth |
| Production-readiness manifest | Missing before this register | Generate one per RC with explicit `proven`/`blocked` fields | Prevents evidence overclaiming |

## Isolated live-source rehearsal snapshot — 2026-10-13

This is a database-restore rehearsal from the legacy 1.0 control plane, performed while the source API remained online. It proves that this point-in-time database dump can be restored into an isolated database on the BugIdea host; it does **not** prove write-freeze, complete profile-object migration, application compatibility, or cutover readiness.

| Check | Evidence | Result |
|---|---|---|
| Legacy source | PostgreSQL 16.15 database `buglogin`; 26 public tables; database size 10,820,631 bytes | Read-only source; 1.0 API remained online |
| Snapshot artifact | Custom-format `pg_dump`, `--no-owner --no-privileges`; 166,661 bytes; SHA-256 `e1341386a5eaf2ce5ff4aa68fe5335e18f0a248d008d70de9331a8f67a080de6`; `pg_restore --list` succeeded | Verified on workstation and both VPS hosts; owner-restricted copies |
| Destination | `buglogin3_rehearsal_20261013` on the isolated BugIdea PostgreSQL instance; restored in one transaction with `--exit-on-error --single-transaction --no-owner --no-privileges` | 26 public tables restored |
| Row-count reconciliation | Exact per-table count comparison between source snapshot and rehearsal database | All 26 tables matched; zero count differences |
| Access isolation | Database ACL grants `CONNECT` only to owner `postgres`; `PUBLIC` and role `buglogin3` cannot connect | Passed; BugLogin 3 API remained inactive |
| Source/service integrity | Source database was not modified; old `buglogin-api` PM2 service remained online | Passed for this rehearsal window |

The source was live during `pg_dump`; PostgreSQL's dump is a consistent snapshot, but there was no application write-freeze and therefore no claim that the dump is the final cutover state. The rehearsal database is intentionally inaccessible to the BugLogin 3 service identity. This artifact covers control-plane PostgreSQL only; S3/profile payloads, key distribution, migration transforms, API-level reads, rollback against a populated destination, and native desktop release evidence remain unproven.

## Legacy control-plane and S3 inventory — 2026-10-13

Read-only queries against the same live 1.0 source found 49 workspaces (37 personal, 12 team), 34 hosted profile rows, 91 memberships, 42 share grants, and 14 workspace groups. The `workspaces` schema has no organization relationship or organization table. Profile runtime metadata is mixed: 21 `bugmini`, 4 `bugox`, and 9 `cloak`. The local 1.1.84 data-root snapshot contains only 14 valid profile metadata rows and is not a complete source for this hosted control plane.

Read-only S3 `ListObjectsV2` inventory used the existing 1.0 server credential without emitting credential values, object keys, or object contents:

| Prefix | Objects | Bytes | Migration interpretation |
|---|---:|---:|---|
| `buglogin-prod` | 6,801 | 858,029,757 | Includes `workspaces`: 6,729 objects / 858,000,006 bytes; candidate production sync data, linkage not complete |
| `profiles` | 10,578 | 1,786,636,841 | Separate root; key layout not yet reconciled to hosted profile IDs |
| `workspaces` | 8,351 | 1,495,646,206 | Separate root; key layout not yet reconciled to hosted workspace IDs |
| `releases` | 164 | 11,242,861,871 | Browser/desktop release artifacts; must not be included in profile migration |
| `selfhost-dev` | 17,679 | 1,848,800,956 | Development/self-host data; must not be included in production migration |
| Other (`meta`, `proxies`) | 25 | 12,241 | Requires explicit ownership/purpose mapping |
| **Total** | **43,598** | **17,231,987,872** | Inventory only; payload snapshot/import not performed |

Within the candidate `buglogin-prod/workspaces` path, only 23 of the 34 database profile IDs and 13 of the 49 workspace IDs appear as exact path segments. These counts do not prove that missing records have no data elsewhere or that matching objects are complete. Relationship validation found one active share grant whose recipient is an unconsumed invite rather than a direct workspace member; the current migration mapper correctly fails closed for that grant. Organization synthesis and pending-invite/grant treatment require explicit deterministic migration rules before a complete export can be produced.

This S3 credential is shared with 1.0 and can list multiple production, development, profile, workspace, and release prefixes. A `buglogin3/prod` write prefix is not an isolation boundary. No S3 payload was downloaded, changed, or deleted during this inventory.

## Still blocked by environment or production controls

### Local PostgreSQL desktop development and current staging gate — 2026-10-19

The local desktop development API now uses a dedicated PostgreSQL 17 database over loopback with TLS required; the client verified the configured local root certificate and negotiated TLS 1.3. Readiness on `127.0.0.1:12345` reports `persistence=postgres`. A local-only platform administrator was provisioned, and `POST /v1/auth/desktop/login` plus authenticated `GET /v1/account` succeeded against that API. The credential file remains outside the repository with an ACL limited to the local user, SYSTEM, and Administrators. No hosted account or database was modified.

The current desktop dev session was restarted through `node scripts/tauri-dev-local-api.mjs`; this passes the launcher readiness/OpenAPI gate, runs Vite HMR at `127.0.0.1:1420`, and launches the Tauri debug app. The first launch exposed a Tauri v2 config error because a window-level `icon` property was unsupported; that property was removed while retaining the bundle icon list. Vite and the Tauri app are currently running. UI credential submission and post-login rendering have not yet been exercised.

The same-day read-only `pnpm release:verify:staging-api` check against `https://api3.buglogin.com` still fails: staging serves 94 paths / 100 operations while source has 97 / 103; staging omits `auth.desktop.login`, `grant.list.legacy`, `grant.list.page`, and `runtime.catalog.get`, advertises obsolete `grant.list`, and omits the `X-BugLogin-Runtime-Catalog` readiness header. No production mutation or deployment was performed. These remain release blockers.

## Direct source-to-target diff — 2026-10-16

The owner requested a single direct migration pass instead of repeated rehearsals. A fresh read-only PostgreSQL count was taken from the live 1.0 `buglogin` database over the existing `kofserver` SSH alias, and compared with the isolated 3.0 `buglogin3` candidate on `bugideas-prod`:

| Surface | BugLogin 1.0 source | BugLogin 3.0 candidate | Result |
|---|---:|---:|---|
| Database size | 10,820,631 bytes | 7,937,047 bytes | Candidate contains schema only |
| Public tables | 26 | 3 (`buglogin_control_audit`, `buglogin_control_idempotency`, `buglogin_control_records`) | Different storage models; direct PostgreSQL restore is not a compatible application migration |
| Users / account records / credentials | 59 / 59 / 59 | 0 control records | All source identity and credential state is absent from candidate |
| Workspaces | 49 | 0 control records | All source workspace state is absent from candidate |
| Hosted profiles | 34 | 0 control records | All source profile metadata is absent from candidate |
| Memberships | 91 | 0 control records | All source membership state is absent from candidate |
| Workspace groups | 14 | 0 control records | All source group state is absent from candidate |
| Share grants | 42 | 0 control records | All source share state is absent from candidate |
| Subscriptions / entitlements | 49 / 49 | 0 control records | All source workspace plans are absent from candidate |

The separate `buglogin3_rehearsal_20261013` database is a 26-table legacy-schema clone, not the 3.0 destination; it must not be mistaken for migrated 3.0 data. This fresh baseline made no writes. It confirms the target is empty and the SQL schemas are incompatible, so the clone must use the 3.0 migration contract, not restore the 1.0 SQL dump over the candidate. The candidate API is healthy, but its protected runtime environment lacks both migration keys and it has no owner identity; package activation is gated by authenticated `migration.write`. A direct deterministic ID comparison between the live hosted profile IDs and the 2026-10-11 local snapshot's profile directories found 13 exact mapped-ID matches, 21 hosted profile IDs absent from the snapshot, and 2 snapshot IDs absent from the current hosted DB. The snapshot inspect report records 14 valid profiles, one profile directory missing metadata, and 88 proxies; all 14 valid profiles are `bugmini` and classified `partial` pending runtime artifacts. This mismatch may include five days of source drift, but it proves the local snapshot alone is not a complete current migration source.

Read-only auth inventory (2026-10-16): the source `users` table has no account-status column; all 59 users have workspace membership and a credential record. The legacy provider column contains 58 `password` accounts and one `password_google` account. The initial Rust exporter omitted password hashes and OAuth subjects, which would have left imported users unable to sign in. The current implementation encrypts Node-compatible scrypt salt/hash material into the signed migration package, creates verified identities and login indexes atomically with control import, and upgrades successful logins to Argon2id. Targeted migration/auth tests are written but unrun; the final encrypted export has not been generated. OAuth subjects remain excluded, and outbound recovery is unsupported.

A fresh read-only S3 listing was performed on 2026-10-16 using the legacy sync service's existing in-memory configuration; no credentials or object keys were printed. The active sync config confirms bucket `mavelnut-login`, root prefix `buglogin-prod`, and no workspace-prefix template. Scanning that entire configured root found 6,801 objects / 858,029,757 bytes, with exact path segments for 23 of 34 hosted profile IDs and 13 of 15 local-snapshot UUID directories. Its `workspaces/` subtree contains 6,729 objects / 858,000,006 bytes; the other 72 objects under the configured root add no further exact profile-ID matches. The legacy `profiles/` prefix contains 10,578 objects / 1,786,636,841 bytes but no exact hosted profile/workspace ID segments; `workspaces/` contains 8,351 objects / 1,495,646,206 bytes, with one exact hosted profile ID and four workspace IDs. Thus the other legacy roots do not close the profile/workspace mapping gap by exact identifiers. No S3 object was read, copied, or modified. No migration write was performed.

A second path-ID-only pass confirmed the older `profiles/` and `workspaces/` roots contain 42 and 38 UUID-like path IDs respectively, but none match either the local snapshot's 15 UUID profile directories or the 34 normalized hosted profile IDs. The candidate `buglogin-prod/workspaces/` prefix contains 91 UUID-like path IDs, including 13 local snapshot IDs and 23 hosted profile IDs. Those prefixes therefore cannot be joined by exact profile ID alone; any attempt to treat the unmatched root objects as profile backups would require an independent, validated manifest or owner mapping. No object bodies were fetched.

Combining the exact source ID intersections gives the current direct diff: 13 hosted profiles have both a local snapshot directory and a matching S3 path ID; 10 more have an S3 path ID but no matching local snapshot directory; 11 hosted profiles appear in neither source. A path-ID match proves only that an object key references that profile, not that its complete object graph is intact; object bodies and manifests remain unverified.

Migration package source audit (2026-10-16; code re-inspected 2026-10-16): the live control DB has 32 active profile-share grant rows; 31 rows target a profile ID with no local snapshot metadata, across 24 distinct profiles. The current mapper merges authoritative `workspace_profiles` rows and emits the resource/binding graph, so the earlier finding that the package could not represent hosted-only profiles is resolved in code (tests remain unrun). Hosted browser bytes do not go through package-level `SyncExport`: after control import, the one-pass desktop migration action reads the prepared `profiles/<uuid>/profile/**` trees and commits each profile through the normal authenticated lease, encrypted snapshot, blob upload, and manifest-commit path. This keeps plaintext browser data on the user's machine and makes hosted profile data available through regular sync. The Tauri importer and API integration are present in source but remain unbuilt and unverified; the direct migration is not production-ready until the consolidated check and one import prove that path.

Legacy sync manifest read-only audit (2026-10-16): the 1.0 sync service source confirms profile manifests at `workspaces/{workspaceId}/profiles/{profileId}/manifest.json` under its configured S3 root, with file objects under `.../files/{relativePath}`. Using its configured S3 client, 57 manifests were fetched and parsed without downloading profile files. All 57 parsed; none declared E2E encryption. Their manifests describe 6,516 files / 782,065,659 bytes. A read-only join against the 34 hosted DB profiles found 23 manifests under the correct workspace/profile ID path; each manifest's `profileId` exactly matches its path and database `prf_...` ID. Those 23 describe 3,467 files / 354,148,555 bytes; all referenced object keys exist in the S3 listing, but five object sizes differ from their manifest sizes (expected/actual bytes: 172/340, 45,773/46,350, 17,935/17,876, 3,323/4,381, and 85/86). Manifest paths were relative and contained no traversal components. 11 hosted profiles still have no matching manifest. The manifest uses 64-character BLAKE3 hashes as defined by the 1.0 sync implementation. Object bodies were not fetched, checksums were not verified, and nothing was copied or modified. The 23 profiles are therefore payload-present candidates, not yet payload-verified migrations.

The 11 hosted profiles without an exact object path in `buglogin-prod/workspaces/` are not all safely classifiable as empty: a read-only metadata join found five with `last_used_at` and five with a proxy assignment. Their runtime counts are six `bugmini`, four `bugox`, and one `cloak`. The five unmatched profiles with activity were last used between 2026-04-09 and 2026-05-11; none was used in the 30 days before the audit, and all were used within the prior 365 days. Do not silently omit or create them as empty profiles during the one-pass copy; their payload source or an owner-approved exclusion must be resolved in the source-to-target diff. The active local `%LOCALAPPDATA%\BugLogin` profile root still has 15 UUID directories / 14 metadata files and its root timestamp is 2026-05-28, matching the old incomplete snapshot rather than supplying the 21 hosted IDs absent there.

| Gate | Why offline fixtures cannot prove it | Required artifact |
|---|---|---|
| Production-clone export | A stable local BugLogin 1.1.84 profile-root clone was inspected, but it has only 14 profile metadata rows versus 34 hosted profile rows. The live control schema has no organizations table, one active grant targets an unconsumed invite, and only 23/34 profile IDs plus 13/49 workspace IDs appear in the candidate S3 prefix | Approved deterministic organization and invite/grant mapping rules; immutable combined source snapshot; complete S3/profile/control-plane reconciliation; signed package and before/after source hash |
| Source write freeze | Requires the old client/server write path and an approved freeze window | Rejected-write probes, timestamps, operator sign-off |
| One-pass production clone | Requires the final source snapshot and the single destination import | One source-to-target diff, one signed package, one post-import reconciliation, and a retained rollback checkpoint |
| Hosted PostgreSQL/Redis/S3 migration path | Requires trusted staging endpoints and real adapter coordination | Transaction, lease, recovery, and orphan-GC evidence |
| Production rollback checkpoint | Requires destination pre-state and a cutover journal from a real clone | Signed checkpoint, rollback result, post-rollback reconciliation |
| Windows/macOS migration smoke | Requires native signed/notarized artifacts | Platform-specific launch, profile, proxy, cookie, extension, sync results |

These blocked gates remain blocked. Synthetic tests must not be relabeled as production evidence.

## RC evidence bundle

Each sanitized RC run should archive only:

1. `manifest.json`: schema, evidence class, run ID, commit, tool version, fixture IDs.
2. `source.json`: source identity and tree hash; no absolute source path.
3. `package.json`: package ID, checksum-manifest hash, signature verification result.
4. `activation.json`: expected/created/reused counts and idempotency result.
5. `reconciliation.json`: counts, hashes, graph checks, ACL result, exclusion result.
6. `rollback.json`: activation rollback, cutover rollback, repeat behavior, retained foreign artifacts.
7. `redaction.json`: secret scan result and forbidden-string result.
8. `commands.txt`: exact commands, exit codes, and test summary.

Required manifest values for sanitized runs:

```json
{
  "schema": "buglogin.migration-evidence/1",
  "evidenceClass": "synthetic-only",
  "productionReadiness": "not-proven",
  "sourceAccess": "sanitized-fixture",
  "secrets": "absent",
  "sourceMutation": "not-observed",
  "hostedTransactionality": "not-tested",
  "rollback": "passed"
}
```

`productionReadiness: ready` is forbidden for this evidence class. A production bundle may use that value only after every blocked gate above has independently passed and received operator approval.

## Suggested offline commands

```text
cargo test -p buglogin-migration --test rehearsal
cargo test -p buglogin-migration --test rollback_retry
cargo test -p buglogin-migration --test fixtures
cargo test -p buglogin-migration --test cli
```

The commands produce test evidence only. They do not access `E:\bug-login`, any production database, hosted object storage, or live user data.

## Direct clone execution update — 2026-10-16

Source SSH access and the 1.0 PM2 API process are available; source PostgreSQL and S3 configuration are present. The isolated 3.0 API is active with 59 GB disk free. A distinct random migration encryption/signing pair was created in the local Windows Credential Vault and copied into `/etc/buglogin3/api.env` through an atomic mode-0600 update; key values were not emitted. A read-only source query found 49 workspaces and no source storage/quota columns. Export inspection found 28 active profile-share rows referencing 22 absent profile IDs and 29 active share rows addressed to platform administrators. These are being retained as hashed, inert `orphaned_profile` or `platform_recipient` provenance; they will not become 3.0 permissions. No source or destination business records have been changed. A 1 GiB per-workspace initial storage cap is being used for the one-pass import because the source schema has no storage quota field; workspace policy remains adjustable in BugLogin 3.0.

### One-pass clone result — 2026-10-16

The source PostgreSQL export used a read-only repeatable-read transaction. Its signed control package (`baadf657-8d44-54b6-b75f-d422a77ad222`) passed signature/checksum verification locally; all transferred package file checksums passed again on the 3.0 VPS. The production candidate was empty before import. A mode-0600 database snapshot was retained at `/var/lib/buglogin3/migration-snapshots/pre-import-20261016.dump` (SHA-256 `3569d0c23ced41a718be4871c639e52ee6867c193b336a60027593b6dfdde5d9`).

| Entity / relation | 1.0 source | 3.0 created | Diff / treatment |
|---|---:|---:|---|
| Accounts with encrypted legacy password credentials | 59 | 59 | 0 |
| Organizations / workspaces | 49 / 49 | 49 / 49 | 0 |
| Membership source rows | 91 | 83 workspace memberships | 8 platform-scope rows map to account-level platform roles; they are not workspace memberships |
| Platform administrators | 4 | 4 | 0 |
| Teams / groups | 12 / 14 | 12 / 14 | 0 |
| Profiles / resources / profile-resource bindings | 34 / 34 / 34 | 34 / 34 / 34 | 0 control-plane records; browser data is a separate authenticated sync step |
| Share mapping | 42 source rows | 16 direct grants + 16 group-grant records + 34 inert provenance rows | Eight source shares are represented as grants (4 active profile, 3 active group, 1 revoked group); the group rows expand to 12 active and 4 revoked actions. The provenance rows cover 29 platform recipients and 28 orphan profiles with overlap |

The direct import receipt is `active`, `replayed=false`, with zero reused records. The API was restarted on the new Linux release binary; `/v1/health` returns `ok` and `/v1/readyz` returns `ready`. No writes were made to the 1.0 source.

S3 hydration fetched 3,338 files / 341,944,745 bytes from 23 manifests. Payloads for 22 profiles are staged on G:. Eleven hosted profiles had no matching manifest and one failed integrity validation; these 12 remain preserve-only. The 22 available browser payloads, including cookies within the browser file sets, have **not** yet been imported to 3.0: the migrated account must sign in to the desktop client and run `migration.profile.data.import`, which encrypts and uploads through normal sync. Therefore the control-plane clone is complete, but browser-data clone and end-to-end usability remain incomplete. The inspection report also found `bugmini:partial` and `cloak:unsupported`; those affected profiles remain non-launchable until their runtime/data is resolved.

### Direct payload inventory confirmation — 2026-10-17

Read-only inspection of `G:\\buglogin-migration-20261016\\prepared-source` confirms the staged export still contains 34 control-plane profiles and 22 corresponding browser-data trees. The browser trees contain exactly 3,338 files / 341,944,745 bytes, matching the hydration record above. The profile subtree also contains 22 `metadata.json` files (445–474 bytes each), so `profiles/` totals 3,360 files / 341,954,145 bytes. The prepared-source root additionally contains `control-plane-export.json` (286,783 bytes) and `version.json` (151 bytes); the actual full tree therefore totals 3,362 files / 342,241,079 bytes. The prior full-tree count omitted those two root files. No BugLogin desktop process is currently running, and the visible BugLogin session is at `app3.buglogin.com/login` with blank credentials, so no authenticated desktop sync session is available to submit the import or obtain its completion receipt. The 22 profiles remain staged, not confirmed in 3.0 hosted sync. No source or destination data was changed.

Per-profile request sizing was checked read-only on 2026-10-17: all 22 trees together contain 3,360 files / 341,954,145 bytes including metadata; the largest tree is 38,878,241 bytes across 154 files. The desktop imports profiles one at a time through ordinary encrypted sync, so every current profile tree is well below the API's 320 MiB per-request limit. No upload-size configuration change is needed for this dataset.

### Referenced proxy preparation and import wiring — 2026-10-17

The prepared control export contains 20 distinct non-empty profile `proxyId` references. Its `proxies/` directory contains 20 matching JSON records; a read-only identity-set comparison found zero missing referenced files. The desktop migration operation now checks every target profile's mapped proxy assignment against the source, resolves the active target workspace from its binding, then creates or reconciles the mapped proxy through the normal authenticated REST mutation and local OS credential vault before importing browser files. Existing mismatched metadata, an endpoint deduplication collision, or incomplete credentials fail that profile closed. The API receives proxy metadata only; credentials enter the existing desktop vault/sync flow. This is source and prepared-input evidence only: no proxy was written to the 3.0 API, and the importer remains unbuilt, untested, and not authenticated against the live candidate.

### Current 3.0 control/sync state — 2026-10-17

A fresh read-only query over SSH to the isolated BugIdea-hosted `buglogin3` PostgreSQL database confirmed 59 `account`, `account_identity`, `account_identity_email`, `account_identity_username`, and `account_password` records. The persisted control aggregate contains 49 organizations/workspaces, 83 workspace memberships, 12 teams, 14 groups, 34 profiles, 34 profile-resource bindings, 16 group-share grant records, and four platform administrator accounts. It contains 34 sync manifests; every manifest is still at genesis generation 1 (zero are above generation 1). This confirms the control-plane clone is present and the encrypted browser-payload import has not yet happened. A separate read-only check of the four platform-admin identity records found generated `legacy-…` usernames and preserved email identities; the desktop login accepts username or email, so use the 1.0 account's email and password. Password verification and authenticated desktop sync remain unverified. No password hashes, tokens, session values, or profile contents were read, and no writes were performed.

### Admin identity lookup and local API E2E — 2026-10-19

A fresh read-only SSH query of `buglogin3.public.buglogin_control_records` confirmed four durable platform-admin IDs and mapped each to its preserved email identity through the account-identity records. The owner's known email is among them. The dedicated `buglogin-admin-password-reset` executable is still absent from `/opt/buglogin3`; no admin password, hash, session, or token was read, and the query made no database writes. This identifies the candidate login accounts but does not prove any password works.

Built the current API and local bootstrap binaries into the external C: target cache (`C:\buglogin-e2e-source-current`), then ran three isolated Node process E2Es sequentially against synthetic SQLite data. All three passed: login/credential rejection/refresh rotation and replay revocation/profile CRUD/restart persistence; organization/workspace switching with tenant-scope isolation and profile/proxy CRUD visibility; encrypted cross-device profile/proxy/credential sync, AES-GCM decryption and wrong-key/tag rejection, manifest commit/read/checksum, and API restart persistence. The Rust `http_auth` integration suite passed 15/15, and the full `buglogin-sync` suite passed 75/75. Two failing sync assertions were stale fixtures: the object-reuse fixture lacked required v2 payload-encryption metadata, and a renamed chunk path did not update its path-bound object keys. The fixtures were corrected without changing production logic. The contract checker passed with 116 command IDs. This does not verify hosted PostgreSQL/S3/Redis, production admin login, Tauri UI, or a deployed artifact. An initial E2E run failed because the harness attempted to replace the API executable currently locked by the desktop process in `C:\buglogin-target`; rerunning with `CARGO_TARGET_DIR=C:\buglogin-e2e-source-current` avoided touching the live desktop/API processes. No production services or data were changed.

A public read-only recheck on 2026-10-19 found `api3.buglogin.com/v1/readyz` and `/v1/openapi.yaml` return 200, but the served contract is missing `auth.desktop.login`, `runtime.catalog.get`, and `grant.list.page`; all three are present in the current source contract. No authenticated request or production write was made. The production candidate still needs a reviewed deployment of the current source before the desktop API flow is compatible.

The Tauri local API launcher now accepts `BUGLOGIN_DEV_API_BASE_URL` only as an explicit loopback HTTP origin, and a configurable `BUGLOGIN_DEV_CARGO_TARGET_DIR` permits a separate C: build cache; Windows F: targets are rejected. Its contract suite passes 8/8, including the canonical source OpenAPI and rejection of stale contracts, remote origins, and F: targets. The current-source API process E2E invokes the launcher's read-only `--check` against a dynamic alternate port and passes. In addition, a source-current API process is running locally at `127.0.0.1:12345` against isolated SQLite data in `C:\buglogin-local-current-20261019-02`; the launcher check, direct desktop login, and authenticated `/v1/account` all passed. The older desktop/API processes were not stopped. A full alternate-port Tauri session has not yet been started.

### Live 3.0 API contract drift — 2026-10-17

Read-only GETs to `https://api3.buglogin.com/v1/health` and `/v1/readyz` returned 200 (`ok` / `ready`). On 2026-10-17, the served `/v1/openapi.yaml` returned 151,476 bytes with SHA-256 `5763cf4f5ae35ff892a254a8c87d7936180d64d1d24a1aef4573a7cca21b5673`; the current worktree file is 156,847 bytes with SHA-256 `d0a7738d37c0ecff52a9980d18057fb395fc6684fa1c3ba46160ad17bc9d496e`. Parsed YAML contains 119 live operations and 120 local operations. The sole local-only operation is `GET /v1/runtime/catalog`; there are no live-only operations. Twenty-one shared operation objects and fourteen shared schemas differ structurally. The live `servers` URL still names `https://api.buglogin.com`, while local source names `https://api3.buglogin.com`. Live `ProfileCreateRuntime` still advertises `bugox`, `bugium`, `bugthor`, and `bughelium`; local source permits only `wayfern` and `bugmini`. The internal live `ProfileDeleteInput` schema does not require the confirmation field, but the actual REST `ProfileDeleteBody` on both live and local contracts requires the exact `QUARANTINE_PROFILE_DATA` constant; the previous statement that production omitted the REST confirmation was incorrect. No authenticated mutation or deployment was performed. API health is not proof that current source is deployed or that authenticated profile/sync flows work. This comparison supersedes all earlier same-day operation counts, hashes, and deletion-confirmation conclusions.

### Production desktop availability — 2026-10-17

Headless browser inspection of `https://app3.buglogin.com/` showed the public web portal; `/login` is the hosted sign-in page. `/download` returned the production state “No verified release is available” and says the release catalog has not published a downloadable build. The desktop profile manager is a separate Tauri application and is not available from the production site today. This confirms the immediate user-visible blocker is desktop distribution, in addition to unverified owner login and profile/sync flows. No login attempt, data mutation, or deployment was performed.

### BugLogin web deployment — 2026-10-16

The `buglogin3-web.service` Next.js standalone release was built from the current worktree with two build workers and output redirected to G:. OpenAPI `anyOf` unions now generate concrete TypeScript unions instead of `unknown`; this allowed the web build typecheck to catch and correct existing profile/proxy type errors. The Linux payload was assembled without following Windows junctions and transferred with SHA-256 verification. Release `/opt/buglogin3-web/releases/20261016-04` is active through the atomic `current` symlink; `/opt/buglogin3-web/releases/20261016-03` and `/opt/buglogin3-web/releases/20261014-02` remain available for rollback. The deployed login preserves `/device` as the return path after a successful sign-in, so users can approve desktop authorization without manually navigating back. Local checks for `/login`, `/device`, and proxied `/v1/readyz` returned 200. A raw-byte read-only comparison confirms the live API OpenAPI document exactly matches the source (94 paths, 119 operations, SHA-256 `5763CF4F5AE35FF892A254A8C87D7936180D64D1D24A1AEF4573A7CCA21B5673`). The web login permits legacy passwords shorter than 12 characters; this has not been exercised with an actual migrated account. Payload SHA-256: `A542B0E6E685512C2A72D6CC637E67B28505775F9BD24AD7A4FC7D218052F921`. This is not complete release acceptance: user login, desktop device approval, and encrypted profile-data import remain outstanding.

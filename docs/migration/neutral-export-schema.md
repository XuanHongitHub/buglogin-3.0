# Neutral Export Schema

- Media type: `application/vnd.buglogin.export+zip`
- Schema: `buglogin.export/1`
- Encoding: UTF-8 JSON/JSON Lines; binaries stored separately
- Archive: ZIP64, deterministic paths, no absolute paths, no symlinks

## Package layout

```text
manifest.json
schema/manifest.schema.json
entities/profiles.jsonl
entities/profile-resources.jsonl
entities/groups.jsonl
entities/proxies.jsonl
entities/vpns.jsonl
entities/extensions.jsonl
entities/extension-assignments.jsonl
entities/workspaces.jsonl
entities/memberships.jsonl
entities/grants.jsonl
sync/export.json
objects/sha256/<first-two>/<remaining-hash>
reports/source-inventory.json
reports/id-map.jsonl
reports/migration-results.jsonl
reports/residual-risks.json
checksums.sha256
signature.json
```

Empty entity files may be omitted when manifest count is zero. Unknown top-level paths are rejected unless declared optional.

`sync/export.json` uses `buglogin.sync-export/1`. It binds one workspace/resource generation to a canonical manifest checksum (fencing token excluded), deterministic blob-set checksum, ordered generation chain, complete object bytes, and retained tombstones. The package rejects missing, extra, duplicate, or tampered blobs. Import never preserves the source fencing token; hosted authority issues a new token after ACL, binding, and lease validation. Current implementation preserves and verifies this envelope inside the signed migration package. Hosted atomic apply and rollback remain release blockers.

## Manifest

```json
{
  "schema": "buglogin.export/1",
  "package_id": "uuid",
  "created_at": "RFC3339 UTC",
  "created_by_tool": {"name": "string", "version": "semver", "commit": "40-hex"},
  "source": {"system": "buglogin-legacy", "version": "1.1.84", "instance_id": "non-secret", "os": "windows|macos|linux", "architecture": "x86_64|aarch64", "snapshot_id": "string", "snapshot_sha256": "64-hex"},
  "compatibility": {"minimum_importer": "semver", "required_capabilities": [], "optional_capabilities": []},
  "security": {"contains_secrets": false, "contains_cookie_values": false, "encrypted": false, "encryption": null, "redaction_policy": "buglogin.redaction/1"},
  "contents": [{"path": "entities/profiles.jsonl", "media_type": "application/x-ndjson", "records": 1, "bytes": 123, "sha256": "64-hex"}]
}
```

Required properties cannot be null. Timestamps use UTC `Z`. IDs are lowercase canonical UUIDs.

## Entity envelope

```json
{
  "schema": "buglogin.profile/1",
  "source_ref": {"system": "buglogin-legacy", "instance_id": "string", "id": "string"},
  "id": "deterministic destination UUID",
  "revision": 1,
  "created_at": "RFC3339 UTC or null",
  "updated_at": "RFC3339 UTC or null",
  "data": {},
  "extensions": {}
}
```

`extensions` is optional namespaced data. It cannot bypass required validation.

## Core records

Profile required data: `name`, `runtime`, `release_type`, `ephemeral`, `source_data_object`, `migration_class`.

Profile optional data: `group_id`, `proxy_id`, `vpn_id`, `extension_group_id`, `tags`, `note`, `last_used_at`, `dns_blocklist`, `launch_hook`, `fingerprint_config`, `legacy_runtime_config`.

- `runtime` preserves source runtime string.
- `migration_class` is `direct`, `adapter`, `partial`, or `unsupported`.
- `source_data_object` references an object; files are never embedded.
- Proxy and VPN cannot both be assigned.
- Unsupported profiles remain inspectable but not launchable.

Profile-resource binding records use schema `buglogin.profile-resource/1`. Required data: `profile_id`, `resource_id`, `workspace_id`, and `archive_state`; `archive_state` is `active` or `archived`. A managed/shared profile has exactly one binding. Local-only profiles may have none. The binding, never equal strings or display names, is the authority for workspace, grant, lease, and sync joins.

### Exact legacy cloud `prf` mapping

- Accept only a legacy cloud profile ID matching `^prf_([0-9a-fA-F]{32})$`; trim it, lowercase the captured hex, then render `8-4-4-4-12` lowercase UUID text. Example: `prf_00112233445566778899aabbccddeeff` becomes `00112233-4455-6677-8899-aabbccddeeff`.
- Set both destination `profile.id` and the profile resource `resource.id` to that canonical UUID for this legacy source. Emit an explicit `profileResource` record even though these two IDs are equal; importers must never infer the binding from equality.
- Rewrite every legacy cloud profile reference, including `share_grants.resource_id` where `resource_type = profile`, through the same conversion. Preserve the separately mapped legacy `workspaceId` as `workspace_id` and require it to match the destination resource workspace.
- Set `archive_state` to `active` for a live `WorkspaceProfileRecord`. The legacy cloud record has no archive flag and hard-deleted records are not synthesized. Any separately discovered archived/local record requires an inventory-backed rule and must not be guessed into this mapping.
- A malformed `prf` ID, duplicate canonical UUID, cross-workspace reference, or profile grant without one matching binding is a blocking migration result, not a fallback string mapping.

Proxy required data: `protocol`, `host`, `port`. Optional: `credential_ref`, `rotation_url`, and `bypass_rules`. Protocol is `http`, `https`, `socks4`, or `socks5`; hosts contain no credentials. `credential_ref` is a deterministic `CredentialId` and must have exactly one matching authenticated object under `encrypted/proxy-credentials/`; the decrypted vault value uses `{credentialId, username?, password?}` and is never written to a neutral/redacted package.

VPN v2 required data: `name`, `kind`, and vault-backed `config_ref`. `kind` is `wireguard` or `openvpn`; raw config, private keys, certificates, and auth values never appear in entity JSON.

Extension v2 required data: `name`, `package_type`, `artifact_ref`, `artifact_sha256`, and `compatibility`. Legacy `xpi`, `crx`, and `zip` packages are copied without mutation into `artifacts/extensions/<artifact_ref>/<sha256>`; symlinks, missing files, empty packages, checksum mismatch, unsupported types, and dangling group assignments block migration.

ACL records preserve provenance, actor, resource, access mode, timestamps, revocation, and semantic classification. Unknown roles/access modes stop ACL import.

## Object record

```json
{
  "schema": "buglogin.object/1",
  "id": "sha256:64-hex",
  "data": {"path": "objects/sha256/ab/cdef...", "bytes": 123, "sha256": "64-hex", "content_type": "application/octet-stream", "logical_type": "profile-directory-tar|extension-package|vpn-config|sync-object", "encryption": null}
}
```

Profile directories are deterministic TAR streams: sorted paths, normalized separators, fixed archive timestamp, no escaping symlinks, stable metadata.

## Secrets and integrity

- Default package contains no secrets or cookie values.
- Reports contain only `present`, `missing`, `invalid`, or redacted fingerprints.
- Secret-bearing export requires authenticated encryption of the full package or secret objects.
- `contains_secrets: true` is valid only when `encrypted: true`, `encryption` names the supported authenticated scheme, and checksum-bound contents include the corresponding encrypted proxy-credential or VPN-config namespace. Marker/path mismatches fail verification even when checksums were recomputed.
- Encryption parameters must be versioned before implementation; use Argon2id-derived key plus AEAD.
- Passwords, proxy credentials, VPN keys, tokens, cookies, and API keys never appear in logs, filenames, IDs, checksums metadata, or errors.
- Decrypted temporary files use private temp storage and are removed on success/failure/cancel.
- `checksums.sha256` covers every package file except itself and `signature.json`, sorted by path.
- `signature.json` signs the checksum-file hash and identifies algorithm, key ID, signer, and time.

## Import transaction

1. Validate paths, limits, schema, hashes, signature, and secret policy.
2. Parse all records before writes.
3. Build ID map and reference graph.
4. Validate every profile-resource edge: referenced profile, profile-kind resource, and workspace exist; binding/resource workspace IDs match; managed/shared profiles have exactly one binding; profile IDs and resource IDs are each unique within bindings; `archive_state` agrees with profile/resource archive state; profile grants resolve through the bound resource.
5. Reject collisions, dangling required references, unknown required capabilities, ACL gaps, duplicate bindings, and cross-workspace edges.
6. Stage workspaces/memberships, objects/secrets, dependent entities, profiles, resources, profile-resource bindings, then grants; keep staged rows unavailable to reads, leases, sync, and launch.
7. Re-run full graph validation against staged destination IDs and hashes.
8. Promote objects by hash and atomically activate metadata in dependency order: profiles and profile resources remain inactive until their resource and binding are present; grants activate only after the binding graph passes.
9. Write immutable result and reconciliation reports containing per-entity counts plus profile-resource edge counts, archive-state counts, duplicate/dangling/cross-workspace counts, and grant-to-binding coverage.
10. Re-import changes nothing unless merge policy allows a newer revision.

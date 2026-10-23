# Preserve-Only Profile Authority

## Scope

Import legacy profiles whose runtime is unsupported or not yet approved without activating them as launchable `Profile` entities. Preserve their encrypted profile data and provenance in a migration-owned namespace.

## Contract

- Active `Profile` storage receives only records with migration class `direct` or `adapter`.
- Unsupported/unknown records are stored under `destination/migration-preserved/profiles/<canonical-profile-id>/`.
- Each preserved root contains `record.json`, `profile/`, and a run marker while staging; `record.json` contains canonical ID, source reference, legacy runtime, migration class, name, and data-object metadata only.
- Preserved roots never enter `LocalRepository`, process launch, proxy/VPN leases, sync writes, or normal profile lists.
- A checksum-bound preserved journal records run ID, package ID, created/reused status, root checksum, and rollback state. Exact repeat reuses matching data; mismatches fail closed.
- Rollback quarantines created preserved roots, removes only migration-owned roots, preserves foreign roots, and is idempotent.
- Encrypted package creation is allowed for supported source versions with valid records even when runtime findings are unsupported/unknown; launch cutover remains blocked and reconciliation reports preserved counts separately.
- Plaintext runtime configuration, credentials, cookies, and profile bytes never enter `record.json`, logs, IDs, checksums metadata, or errors.

## Non-goals

- No automatic runtime adapter approval.
- No launch support for Camoufox or unknown runtimes.
- No UI/API surface for preserved roots before a separate capability contract.

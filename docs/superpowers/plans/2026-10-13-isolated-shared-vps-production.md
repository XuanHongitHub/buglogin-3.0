# Isolated Shared-VPS Production Plan

Status: BugLogin-only PostgreSQL, TLS Redis, and environment files provisioned on the BugIdea VPS; API remains staged but disabled. No BugIdea services, data, proxy configuration, or existing Redis/PostgreSQL instances were changed.

Operational correction (2026-10-13): a `buglogin3-redis.service` unit was also present on the legacy BugLogin 1.0 VPS. It was disabled there without deleting its config/data; the 1.0 API health endpoint remained healthy. The BugIdea VPS remains the only active 3.0 Redis host.

## Phase 1 — read-only inventory

- [x] Locate the existing BugIdea VPS deployment procedure without editing its repository or service configuration. Its deploy script is BugIdea-specific (Git checkout, Laravel migration, PHP-FPM/queue reload) and must not be used for BugLogin.
- [x] Confirm SSH access and capture a read-only host baseline: 2 vCPU, 3.8 GiB RAM, 60 GiB free disk; Nginx, PostgreSQL and Redis are already present; Docker is unavailable. Recheck immediately before any deployment.
- [ ] Identify Nginx vhost ownership, firewall policy, and an internal API bind path without editing shared proxy configuration.
- [x] Provision a separate PostgreSQL database/role without changing BugIdea app data/configuration; verify TLS and the 12-connection cap.
- [x] Provision a separate BugLogin Redis systemd instance using TLS, loopback bind, private CA, namespace ACL/data path, and strict memory/CPU limits. The existing Redis service remains unchanged.
- [x] Install the legacy BugLogin Spaces credential into a root-only 3.0 env file scoped by application prefix `buglogin3/prod` and tenant `buglogin3-prod`; obtain a dedicated bucket-scoped 3.0 key before final production acceptance. Never use BugIdea's storage.
- [ ] Confirm BugLogin-only secrets, monitoring, backup, and restore mechanisms.
- [ ] Select the 3.0 hostname in `buglogin.com`; leave `api.buglogin.com` pointing at 1.0.

## Phase 2 — isolated staging slice

- [x] Add BugLogin-owned native systemd definitions with dedicated service identities and explicit CPU/memory limits; the Redis unit is enabled, while the API unit remains disabled until its artifact and production dependencies are ready. No Docker was installed.
- [x] Provision a separate PostgreSQL database/role with a 12-connection cap and root-only TLS URL file. The Rust adapter now accepts an explicit PEM root through `BUGLOGIN_POSTGRES_TLS_ROOT_CERT`; the API-only trust copy is mode 0640 for the service group. The DB URL uses certificate identity `bugideas-server` with `hostaddr=127.0.0.1`, so the socket stays on loopback while TLS verifies the server name. The live adapter persistence/idempotency probe passed against the staged 3.0 database, and its synthetic rows were deleted.
- [x] Provision the separate TLS Redis service/namespace and root-only application credentials; TLS PING succeeds.
- [x] Verify S3 write/read/delete under the dedicated prefix. On 2026-10-13, the existing Rust `s3_https_adapter_round_trips_checksum_metadata` probe passed on `bugideas-server` using the root-only config and exact `buglogin3/prod` prefix; it performed unique put, idempotent put, checksum/content read, conditional delete, and absence verification. Probe binary SHA-256: `01cd21630a04785a52f392d2f257bb08b30cfebf525642cfc6d348578fd07442`; temporary binary/log were removed. A dedicated 3.0 bucket-scoped key is still required before final production acceptance.
- [x] Prepare root-only production API env and a resource-limited BugLogin-only systemd unit; keep the unit disabled until a Linux artifact and real production recovery-webhook endpoint/secret are available.
- [ ] Define BugLogin-only object-storage backup/retention and quota policy.
- [ ] Deploy only to the dedicated 3.0 hostname and compare deployed OpenAPI bytes/hash with the checked-in Rust contract.
- [ ] Verify readiness, authenticated profile/proxy flows, sync interruption/recovery, and logs/redaction using synthetic data.
- [ ] Record rollback that removes only BugLogin-owned resources and leaves BugIdea service state unchanged.

## Phase 3 — migration rehearsal

- [ ] Snapshot/backup 1.0 source read-only; verify snapshot identity and restore it to an isolated rehearsal source.
- [ ] Dry-run and import the agreed migration cohort into BugLogin 3.0 staging.
- [ ] Compare organization/workspace/member/share graph and profile/browser payload checksums/counts.
- [ ] Rehearse write-freeze, final delta, rollback, and recovery without production writes.

## Phase 4 — release and cutover

- [ ] Complete Windows/macOS release evidence, signing/notarization and artifact provenance.
- [ ] Obtain explicit owner approval for production writes, DNS/ingress cutover, and migration window after evidence is reviewable.
- [ ] Cut over only the selected BugLogin hostname; preserve 1.0 endpoint and rollback window until acceptance.
- [ ] Verify API readiness/OpenAPI, desktop production login/sync, monitoring, backups, and rollback after cutover.

## Current blockers

- Linux API artifact is not deployed; the API unit is intentionally disabled.
- Recovery delivery has no approved HTTPS webhook endpoint/secret; production startup correctly fails closed without both.
- The existing S3 credential is shared with BugLogin 1.0 and is not bucket/prefix-policy isolated. A live canary passed under the 3.0 prefix, but this does not prove credential isolation or backup/restore.
- `api3.buglogin.com` and `app3.buglogin.com` are staged in API env but DNS/tunnel routes are not created or verified.
- Object-store backup/restore, API health/OpenAPI comparison, and monitoring are not verified.
- Hosted sync/auth, migration rehearsal, rollback, native release and signing gates remain open per `docs/readiness/implementation-readiness.md`.

Credential inventory on the legacy 1.0 host (read-only, 2026-10-13) found its S3 credentials and no mail/SMTP/recovery/webhook provider variables in either deployed app env or PM2 environment names. Do not route 3.0 recovery through the 1.0 API; obtain/configure a dedicated delivery provider before enabling API startup.

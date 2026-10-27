# Staging Topology

## Principles

- No production credentials, databases, buckets, signing keys, domains, or user data.
- Staging mirrors production boundaries, not production scale.
- Channels isolate `dev`, `nightly`, `rc`, and `stable`.
- Every service, bucket, database, queue, token audience, deep-link scheme, and feed is environment-scoped.

## Logical topology

```text
Git commit/tag
  -> CI verification
  -> unsigned build artifacts
  -> signing/notarization
  -> immutable artifact registry
  -> staging update feed
  -> Windows/macOS E2E machines

Desktop RC
  -> staging API gateway
      -> identity/account
      -> workspace/control
      -> sync
      -> audit/event
  -> staging object store
  -> staging database
  -> staging queue/workers
  -> staging observability
```

## Isolation

| Resource | Requirement |
|---|---|
| DNS/TLS | Dedicated staging hostnames/certificates; no production aliases |
| Identity | Separate issuer, clients, redirects, token audience |
| Database | Separate project/cluster or hard account boundary; synthetic data |
| Object store | Separate bucket/KMS key; cleanup lifecycle. The current BugLogin VPS temporarily shares legacy BugLogin Spaces credentials; production API startup requires an explicit matching `buglogin3/<environment>` prefix and `buglogin3-<environment>` tenant. This is an application namespace guard, not an S3 ACL boundary. |
| Queue/cache | Separate namespace and credentials |
| Update feed | Separate channel manifest and signer authorization |
| Secrets | Dedicated store; least privilege; short-lived CI identity |
| Observability | Separate project/index; redaction tests |
| Email/webhooks | Sink/test endpoints only |

## Desktop configuration

Artifacts embed only environment, API/sync bases, update channel, telemetry environment, commit, and feature manifest. Secrets are runtime-injected, never embedded.

Server processes require an explicit `BUGLOGIN_ENV=local|production`; a missing or unknown mode must not silently start local in a release deployment. Hosted production must use the `buglogin-api` binary; the secondary `buglogin-control` harness rejects production because it does not configure the S3 and Redis adapters. The Rust API production process requires `BUGLOGIN_API_BIND` to be a valid non-loopback socket address with a nonzero port. Local development retains the loopback default. `/v1/health` is liveness only; ingress readiness must use `/v1/readyz`, which checks PostgreSQL, S3, and Redis. An unrelated `/health` or `/readyz` from a proxy is not evidence that the BugLogin v1 API routes are deployed.

The hosted API's MCP JSON-RPC endpoint is `/v1/mcp` and is disabled unless `BUGLOGIN_MCP_ENABLED=true`. Clients send their own scoped `blapi_` API token as `Authorization: Bearer ...`; the transport validates it against the durable API-token store, then the MCP command policy enforces command scopes, workspace ACLs, idempotency, and audit. Create and revoke these tokens through the account API-token flow. The API process does not keep a second shared MCP bearer secret, so rotation takes effect without restarting it. Never expose tokens in URLs, logs, or source-controlled client config. The default remains disabled in local development and production.

For the current shared-bucket production exception, set `BUGLOGIN_S3_PREFIX=buglogin3/prod` and `BUGLOGIN_TENANT_ID=buglogin3-prod`; both must identify the same application environment. Production refuses the generic `buglogin/v1` prefix and mismatched tenant suffix. Keep the 3.0 environment file root-only and never use BugIdea object storage.

RC artifacts tested in staging are byte-identical to stable artifacts. Promotion changes signed channel metadata, not binaries.

## Rust API release

`Dockerfile.api` builds and runs the Rust `buglogin-api` binary and includes the one-shot `buglogin-bootstrap` and `buglogin-admin-password-reset` operator binaries. `donut-sync/Dockerfile` is a separate legacy service and does not implement the BugLogin v1 API contract. Neither operator binary is an HTTP endpoint. Bootstrap requires `BUGLOGIN_ENV=production`, refuses `BUGLOGIN_DATA_ROOT`, prompts twice for the password without echo, and atomically creates the first owner only when the hosted identity store is empty. It cannot reset imported identities.

For an existing active platform administrator, the password-reset operator binary has a read-only `--list-platform-admins` mode that prints every durable admin account ID, optional username/email, account status, and identity availability as JSON; it prints no password, token, or credential record. The reset mode requires the exact account ID twice, prompts twice without echo, and uses the hosted PostgreSQL configuration. It atomically advances the password version, revokes that account's sessions and API tokens, and writes an audit event. It does not delete account, workspace, profile, or migration data. The current BugLogin 3.0 host uses native systemd and has no Docker; after a reviewed native release stages the executable at `/opt/buglogin3/buglogin-admin-password-reset`, an authorized operator must run it from an interactive terminal with the same root-managed production environment injection used for the API. Keep the password solely in the hidden prompts. Never put it in arguments, environment variables, shell history, logs, or support output. The binary rejects non-production mode and any `BUGLOGIN_DATA_ROOT` setting. The Dockerfile includes the executable for a future container deployment but is not the current host invocation path. This is an access recovery mechanism; it does not establish that a particular imported 1.0 account has a valid identity/password record.

For container hosts, build with `docker build --pull -f Dockerfile.api -t buglogin-api:<commit> .`; inject production PostgreSQL, S3, Redis, device-verification, and web-origin settings through the secret manager, never image layers. The current BugLogin 3.0 VPS has no Docker and runs the isolated API as native systemd. Package Linux x86-64 artifacts with `scripts/release/package-api-linux.ps1`, then use `scripts/release/deploy-api-systemd.ps1 -ArtifactDirectory <outside-repository-path> [-DryRun]`. Packaging and deployment require a clean commit exactly synchronized with its upstream. The staging deploy updates only `/opt/buglogin3/buglogin-api`, `/opt/buglogin3/buglogin-admin-password-reset`, and `buglogin3-api.service`; it snapshots prior executables, checks readiness and exact OpenAPI bytes, and restores the prior binaries if those checks fail. It never changes env files or performs DNS/cutover work. The admin reset tool is installed but never executed by deployment. Outbound account-recovery email/webhook delivery is unsupported for launch; recovery fails closed while delivery is unavailable. Production `BUGLOGIN_REDIS_URL` must use `rediss://`; local development may use `redis://`. The API exposes `/v1/health` for liveness and `/v1/readyz` for PostgreSQL/S3/Redis readiness. A proxy's generic `/health` or `/readyz` is not evidence that the BugLogin v1 API is deployed.

Migration package support is enabled only when both `BUGLOGIN_MIGRATION_ENCRYPTION_KEY_HEX` and `BUGLOGIN_MIGRATION_SIGNING_KEY_HEX` are present. Each value is exactly 64 hexadecimal characters (32 bytes), must not be all zeroes, and the keys must be distinct. The encryption key must match the key used to export legacy account credentials, and the signing key must match the package signer. Store both in the server secret manager and never put them in image layers, URLs, command arguments, or logs. If only one is set or either is malformed, startup fails; if both are absent, ordinary API operation continues and migration endpoints return their explicit unavailable error.

Before switching `api.buglogin.com`, deploy this image to an isolated staging hostname and run `pnpm release:verify:staging-api` (set `BUGLOGIN_STAGING_API_BASE_URL` to override the default `https://api3.buglogin.com`). The gate checks `/v1/readyz`, requires `X-BugLogin-Runtime-Catalog: available`, compares the served OpenAPI bytes with `crates/api/openapi/openapi-v1.yaml`, and reports missing/unexpected operation IDs on drift. The native systemd deployment helper also requires ready status and an available signed runtime catalog before it records an activation. Local API development may still exercise core auth/metadata operations with `runtimeCatalog=unavailable`; that is not a staging deployment or promotion pass. Then verify authenticated REST/MCP behavior, migrations, and failure recovery against staging-only services. This image definition does not itself deploy or authorize a production cutover.

## Native systemd deployment (BugLogin 3.0 staging)

The BugIdea VPS currently has no Docker and runs BugLogin 3.0 through `buglogin3-api.service`. Build the Linux x86-64 binaries with the pinned `BugLogin Native Linux API Artifact` workflow from a pushed commit reachable from `main`. Download the artifact and run `scripts/release/package-api-linux.ps1`; Cosign verifies the keyless Sigstore bundle against the exact repository and workflow identity before it copies the artifact outside the repository. Then use `scripts/release/deploy-api-systemd.ps1 -ArtifactDirectory <outside-repository-path> [-DryRun]`. This release path targets only the isolated BugLogin API unit and files, preserves checksummed rollback copies, and verifies readiness plus exact OpenAPI parity. It does not change environment files, databases, DNS, Cloudflare, or BugIdea services. The included password-reset binary is installed but never executed by deployment. Browser launch remains unavailable until the API reports an available signed runtime catalog.

## Data and failure injection

- Synthetic accounts/workspaces/ACL.
- Migration fixtures and small/medium/generated-large cohorts.
- Controlled proxy endpoints.
- Latency, loss, DNS failure, expired token, revoked grant, checksum mismatch, disk quota, worker restart.
- Secret canaries in sensitive fields.

## Access and retention

Named SSO identities, audited roles, short-lived CI workload identity, automated non-production reset, encrypted short-lived sensitive evidence. Network policy denies production connectivity from staging runners where possible.

## Readiness gate

Staging is release-capable only when isolation tests, seed/reset, observability, update feed, artifact registry, Windows/macOS runners, proxy endpoints, failure injection, and channel-metadata rollback are operational.

## Live endpoint recheck — 2026-10-14

`https://api3.buglogin.com/v1/readyz` and `https://app3.buglogin.com/login` returned 200. A fresh byte-level OpenAPI comparison found staging at 87 paths / 96 operations (SHA-256 `A631002BA749C16F3A0CFE09C9D8D1DB3D40DDD1A78C9820C0A719003D8E16D9`) and this checkout at 91 paths / 101 operations (SHA-256 `D333FE289D941A92F37329207F5EE8331CF080CB626960C7CD0F6BFAD6479130`). Staging is missing `profile.share.keyenvelope.get`, `profile.keyrotation.recipients.list`, `group.share.list`, `group.share.create`, and `group.share.revoke`. `api.buglogin.com/v1/readyz` returned 404 and remains the legacy 1.0 hostname. These were read-only checks; current worktree code has not been deployed.

The `release:verify:staging-api` gate was run against `api3.buglogin.com` on 2026-10-14. Readiness returned 200; OpenAPI returned 200 with 87 paths / 96 operations and the same five missing operations. The current source OpenAPI SHA-256 is `BCCED8FFA984F03EA71DD4190EDD36FC822A076C1B395BA5414C6450E53568C9`; the earlier `D333...` value above described the previous source revision. No staging or production deployment was performed.

Read-only recheck — 2026-10-15: local OpenAPI is 92 paths / 102 operations (SHA-256 `0BC18D228C3109BD2675F69AF7ED5C33EDCDA51E00C5F584FDF826268F8A98E9`); `api3.buglogin.com` serves 87 paths / 96 operations (SHA-256 `A631002BA749C16F3A0CFE09C9D8D1DB3D40DDD1A78C9820C0A719003D8E16D9`). Staging is missing `group.share.create`, `group.share.list`, `group.share.revoke`, `profile.keyrotation.recipients.list`, `profile.share.keyenvelope.get`, and the new `profile.update` operation. No staging or production write/deployment was performed.

Read-only recheck — 2026-10-16: after the isolated 3.0 API and web releases, `api3.buglogin.com/v1/readyz` returns 200. Comparing raw response bytes with `crates/api/openapi/openapi-v1.yaml` yields an exact SHA-256 match (`5763CF4F5AE35FF892A254A8C87D7936180D64D1D24A1AEF4573A7CCA21B5673`); both documents parse to 94 paths and 119 operations, with no missing, unexpected, or renamed operation IDs. `app3.buglogin.com/login` also returns 200 on web release `20261016-04`. Older drift snapshots above are historical and are superseded by this recheck. `api.buglogin.com` remains the separate 1.0 service.

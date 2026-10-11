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
| Object store | Separate bucket/KMS key; cleanup lifecycle |
| Queue/cache | Separate namespace and credentials |
| Update feed | Separate channel manifest and signer authorization |
| Secrets | Dedicated store; least privilege; short-lived CI identity |
| Observability | Separate project/index; redaction tests |
| Email/webhooks | Sink/test endpoints only |

## Desktop configuration

Artifacts embed only environment, API/sync bases, update channel, telemetry environment, commit, and feature manifest. Secrets are runtime-injected, never embedded.

Server processes require an explicit `BUGLOGIN_ENV=local|production`; a missing or unknown mode must not silently start local in a release deployment. Hosted production must use the `buglogin-api` binary; the secondary `buglogin-control` harness rejects production because it does not configure the S3 and Redis adapters. The Rust API production process requires `BUGLOGIN_API_BIND` to be a valid non-loopback socket address with a nonzero port. Local development retains the loopback default. `/v1/health` is liveness only; ingress readiness must use `/v1/readyz`, which checks PostgreSQL, S3, and Redis. An unrelated `/health` or `/readyz` from a proxy is not evidence that the BugLogin v1 API routes are deployed.

RC artifacts tested in staging are byte-identical to stable artifacts. Promotion changes signed channel metadata, not binaries.

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

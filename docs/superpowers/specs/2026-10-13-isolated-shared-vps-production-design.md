# Isolated Shared-VPS Production Design

Status: approved target constraints from owner; deployment implementation and live verification pending.

## Goal

Run BugLogin 3.0 production API in a small, isolated deployment area on the existing BugIdea VPS while preserving every BugIdea service and its data. Keep the current BugLogin 1.0 endpoint and its expiring VPS available for backup and migration rehearsal until cutover is explicitly approved.

## Binding constraints

- `api.buglogin.com` currently serves BugLogin 1.0. Do not repoint it or route 3.0 traffic through it during preparation or rehearsal.
- The old BugLogin VPS is a migration source and backup target only; it is not the 3.0 production host.
- BugLogin 3.0 must have its own deployment namespace, service identity, secrets, logs, health checks, data namespace, and resource limits. Use the host's existing runtime where practical; do not install a new runtime just for BugLogin without a measured need and an isolated change plan.
- BugIdea services, containers, files, databases, object stores, proxy configuration, ingress configuration, firewall rules, credentials, and DNS must not be changed as a side effect of BugLogin setup.
- Shared host use must be bounded. If available CPU, memory, disk, or I/O headroom cannot be proved, stop before deployment.
- Cloudflare CLI and the `buglogin.com` zone are available. Select a dedicated 3.0 hostname and verify its target before creating or changing DNS records.
- Migration reads from the old VPS must be snapshot-based and auditable. No source mutation or production writes during rehearsal; use write-freeze only for an approved final migration window.

## Service boundary

```text
Cloudflare DNS/TLS (dedicated BugLogin 3.0 hostname)
                 |
  BugLogin 3.0 ingress route
                 |
  isolated BugLogin API service (systemd/resource limits)
        |          |           |
 separate DB   separate S3   separate Redis
 namespace     bucket/prefix namespace/credentials

BugIdea services and data: unchanged, separately monitored
BugLogin 1.0 VPS: backup/migration source until retirement
```

## Acceptance requirements

- Inventory and record BugIdea VPS baseline before changes: running services, container/network/volume names, listening ports, ingress ownership, disk, RAM, CPU and backup policy.
- Use a separate Compose/project namespace or equivalent and dedicated network, service account, secret set, DB role/database, object-store bucket or prefix with policy boundary, and Redis namespace/credentials.
- PostgreSQL TLS must validate both trust and server identity without changing the host-wide trust store. If the local PostgreSQL certificate is private/self-signed, BugLogin may trust its public certificate through a BugLogin-only CA path and connect using the matching DNS identity plus a loopback `hostaddr`.
- Do not edit or restart BugIdea-owned services. A BugLogin-only deploy/rollback must target only BugLogin-owned resources.
- Run BugLogin API readiness through its own `/v1/readyz`; fetch `/v1/openapi.yaml` and compare bytes/hash with the repository contract.
- Prove backup/restore for BugLogin 3.0 data independently of BugIdea backup/restore.
- Rehearse migration from a read-only 1.0 snapshot into isolated 3.0 staging; prove counts, relationships, sample profile/browser data integrity, write-freeze procedure and rollback.
- Keep 1.0 DNS unchanged until 3.0 production acceptance and explicit cutover approval. Document reverse DNS/ingress rollback and data write handling.

## Explicitly not assumed

The 2026-10-13 read-only SSH inventory confirmed access, 2 vCPU, 3.8 GiB RAM (2.6 GiB available at observation), 60 GiB free disk, Nginx on 80/443, local PostgreSQL on loopback, and Redis on loopback. Docker is unavailable. This favors a dedicated native systemd API service over installing Docker. A second, BugLogin-only Redis service may use the existing Redis binary with TLS, bind only to loopback, its own ACL/user/data directory, strict memory/CPU limits, and a private CA explicitly trusted by the API. Do not alter the existing Redis service. Production config still requires S3; the legacy BugLogin Spaces credentials can access the old BugLogin bucket, so staging may use a distinct `buglogin3/prod` prefix and tenant ID, but this does not isolate 3.0 credentials from 1.0. A bucket-scoped 3.0 access key remains preferable before final production acceptance. This is a point-in-time sample, not a capacity guarantee. No BugIdea files, configuration, database, services, or data were changed.

The inventory does not prove S3 availability, Nginx vhost ownership, firewall policy, resource quotas, backup/restore isolation, database/Redis namespace isolation, or available signing/deployment credentials. These require read-only evidence before provisioning.

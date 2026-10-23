# Web Auth Local Stack Evidence

Date: 2026-07-23
Status: non-production local smoke evidence.

## Topology

- Rust API: local control store, `127.0.0.1:12343`.
- Next web host: `127.0.0.1:12344`.
- Next server-side `/v1/:path*` rewrite targets the Rust API.
- React uses same-origin URLs; no browser-visible API base URL or bearer token.
- Docker not used.

## Checks

| Check | Result |
|---|---|
| `GET /login` | HTTP 200 |
| `GET /device` | HTTP 200 |
| `GET /v1/health` direct API | HTTP 200 |
| `GET /v1/health` through web proxy | HTTP 200 |
| Test listeners cleaned | Pass |

## Scope

This proves local process topology and same-origin proxy wiring only. It does not prove production PostgreSQL/S3/Redis, TLS, signing, macOS, or migration cutover readiness.


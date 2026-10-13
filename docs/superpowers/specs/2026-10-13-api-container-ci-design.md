# Rust API Container CI Gate

## Goal

Make the supported Rust API image an exercised build artifact in the required CI check, so the `api.buglogin.com` implementation cannot drift from the local Rust/API contract unnoticed.

## Acceptance

- CI builds `Dockerfile.api` from the repository root with the pinned project toolchain.
- The image starts in isolated local mode and serves `/v1/health` and the canonical `/v1/openapi.yaml` bytes.
- The same image fails closed in production mode when PostgreSQL configuration is absent.
- The gate uses only synthetic local state; it receives no secrets and makes no external writes.
- The aggregate CI status job requires this gate.

## Out of scope

Deploying or routing `api.buglogin.com`, provisioning staging credentials, and production database/object-store rehearsal remain separate release gates.

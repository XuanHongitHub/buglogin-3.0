# Local Development Admin Provisioning Plan

Spec: `docs/superpowers/specs/2026-10-19-local-dev-admin-provisioning-design.md`.

- [x] Add local-only atomic provisioning and password recovery through the API authority, guarded to debug/local/loopback PostgreSQL.
- [x] Add a debug CLI using PostgreSQL with mandatory TLS and a local trust root; it generates a random password and emits it only after the transaction succeeds.
- [x] Run the storage TLS configuration test and build the API and admin CLI.
- [x] Migrate the active SQLite control records, idempotency receipts, and audit rows from a verified snapshot into the dedicated local PostgreSQL database.
- [x] Provision a local platform admin and verify desktop login, account read, organization read, and PostgreSQL readiness through the primary API on port 12345.
- [ ] Run the full `buglogin-api` suite, `pnpm check:api`, and `pnpm test:contract`.

## Risks and boundary

- PostgreSQL local TLS uses a machine-local root CA and server certificate under `C:\ProgramData\BugLogin\pg-tls`; its private key is readable only by the PostgreSQL service, SYSTEM, and Administrators.
- The `12345` API process now uses the dedicated `buglogin_local` PostgreSQL role with `sslmode=require` and the local certificate root. A rollback snapshot of the prior SQLite control store is retained.
- This does not create or modify production accounts and does not change the production admin reset procedure.

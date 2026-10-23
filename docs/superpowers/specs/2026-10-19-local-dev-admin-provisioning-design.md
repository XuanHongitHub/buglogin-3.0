# Local Development Platform Admin Provisioning

## Goal

Allow a developer to add a second login-capable platform administrator to an existing local BugLogin API database when one-time owner bootstrap has already run and the original local password is unavailable.

## Scope and safety boundary

- Expose the operation only through a dedicated Rust development CLI. Do not add an HTTP, MCP, Tauri, or production operator route.
- Require a debug build, `BUGLOGIN_ENV=local`, an explicit local PostgreSQL URL using loopback TCP and `sslmode=require`, and a TLS root certificate. Refuse remote PostgreSQL, production mode, SQLite mode, and release builds.
- Generate a random password in the CLI; never accept credentials in argv or environment variables. Print it once only after the durable transaction succeeds; redact it from errors and audit.
- Require an existing active organization and workspace. Create an active account identity, initial password, `PlatformAdmin` role, and active workspace-owner membership in the existing default workspace.
- Persist all account, identity indexes, password, membership, platform-admin role, and metadata-only audit record in one revision-checked control transaction.
- Refuse duplicate canonical username/email, malformed existing aggregates, unavailable workspace, or revision conflicts without partial writes.
- Do not alter the existing owner, profiles, proxies, sessions, or production data.

## Verification

- Unit/integration coverage for local-only environment guards and atomic admin provisioning.
- Verify the new identity can authenticate, receives `PlatformAdmin`, can list the existing organization/workspace, and can read workspace-scoped state after reopening the PostgreSQL store.
- Verify a second invocation does not create a duplicate identity and invalid environments fail before opening storage.
- Run the full API crate suite and relevant contract checks.

# Atomic Profile Proxy Assignment Plan

## Status

Backend implementation verified locally. Production readiness is still gated on hosted PostgreSQL/S3 rehearsal, native release evidence, and the approved UI packet.

## Phases

1. [x] Add canonical DTOs, command registry schemas, OpenAPI/adapter parity, and a persisted pending-assignment service.
2. [x] Add REST and MCP set/get adapters with workspace authorization, revision CAS, in-use/running guards, idempotency, audit, and replay tests.
3. [x] Make manifest commit validate pending assignment and atomically update profile revision, active manifest, audit/idempotency state, and consume pending assignment.
4. [x] Wire the desktop stopped-profile sync flow to load pending proxy selection and expected profile revision, resolve credentials only from OS vault, and reconcile ambiguous commits.
5. [x] Run API, MCP, desktop compile, contract-generation, sync-interruption, secret-boundary, and concurrency checks locally.

## Evidence and remaining production gates

- 2026-10-17 follow-up: REST/MCP now return a stale pending assignment to its authorized owner, and the desktop drawer identifies its source revision so it can be restaged against the current one. Profile sync activation remains revision-strict. REST/MCP regression cases are written; execution is deferred to the consolidated pass requested by the owner.

- `cargo test --locked -p buglogin-api` passed; follow-up unit, REST-profile, and MCP proxy-assignment tests also passed after adding atomic rollback/reopen coverage.
- `cargo check --locked -p buglogin-desktop-app` passed.
- `pnpm test:contract` passed with 97 canonical commands across REST, MCP, Tauri, and generated clients.
- OpenAPI-generated TypeScript DTOs and the web REST client now expose assignment read/stage operations; `pnpm --filter @buglogin/web test` and `pnpm check:api` pass. The stage method name reflects that the active profile changes only after sync commits.
- Atomic test proves mismatch leaves profile, manifest, and pending assignment unchanged; success updates profile revision and manifest together, consumes the assignment, and survives API-state reopen.
- Still required: PostgreSQL + S3 staging rehearsal, interrupted/ambiguous hosted commit rehearsal against production adapters, Windows/macOS release evidence, and a UI implementation packet with approved IA/state/reference/screenshot gates. Local memory-store tests do not substitute for those gates.

## Acceptance

- A failed or stale commit never changes the effective proxy or current manifest.
- A successful commit changes the profile proxy and manifest together and removes the pending assignment.
- Existing manifest commits without pending assignment continue only when proxy dependencies match the active profile.
- [x] Keep stale staged assignments readable through REST and MCP so the owner can see/restage the pending choice; sync activation continues to reject stale revisions. REST and MCP source paths return the stored assignment with its original expected revision; sync activation remains revision-strict. Regression source is present; consolidated verification is deferred.
- Missing target proxy, cross-workspace proxy, wrong profile revision, running profile, missing vault credential/envelope, stale manifest, and lost-response replay all have explicit tested outcomes.
- Generated API clients and REST/MCP/desktop adapters agree with the canonical registry.
- This plan alone does not prove production readiness; PostgreSQL/S3 staging and release evidence remain required.

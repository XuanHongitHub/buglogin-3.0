# BugLogin Production Fast MVP — Scope and Acceptance Design

Status: proposed execution scope; implementation and release remain incomplete.

## Objective

Deliver BugLogin as a usable desktop product backed by the canonical API and PostgreSQL, with browser profiles that install and launch a real approved browser runtime, secure account/workspace behavior, multi-device synchronization, team locks, the promised Extensions/Groups/MCP API surfaces, and a controlled production release.

The work order is fixed:

1. Close the minimum desktop product flow: app startup, sign-in, workspace selection, profile/proxy/group operations, persistence, and recovery states.
2. Make browser runtime download, installation, and profile launch work from the app.
3. Add BugLogin-owned application auto-update.
4. Close account authorization, multi-device sync, and team-lock behavior.
5. Close API-backed Extensions and MCP operations that are part of the MVP menu; keep Groups in the core profile flow.
6. Release to production after evidence and rollback are ready.

## Product and architecture constraints

- Rust/API is authoritative for identity, permissions, persistence, profile state, sync, team locks, runtime catalog publication, and MCP operations. React only presents those states.
- Desktop and hosted environments use the same REST contracts and PostgreSQL data model. Test-only in-memory adapters do not count as hosted-flow evidence.
- A browser is launchable only when the app has installed the profile's exact runtime version through the signed catalog flow. No direct executable override, unsigned fallback, guessed checksum, or fake download state may mark a runtime available.
- The runtime release source must be HTTPS. BugLogin pins an Ed25519 public key in the desktop build; the API serves only a catalog verified against its configured public trust set. The artifact itself is bounded, SHA-256 verified, safely extracted, and rollback-capable.
- Current upstream Wayfern metadata (`https://donutbrowser.com/wayfern.json`) provides version and download URLs but no SHA-256. A release pipeline/operator must obtain the package, calculate its hash, validate its version/platform/executable/source and distribution approval, then sign the BugLogin catalog entry. The multipart CDN ETag is not a checksum.
- Browser artifact staging, extraction, installed runtime, receipts, and rollback must fit the product's supported configurable storage location. Before install testing, record free capacity against the actual package size plus extraction and rollback headroom; do not assume the system drive or rely on an undocumented junction.
- Preserve user profile data and uncommitted work. Any DB migration or production write requires a snapshot, rehearsal, verification, and rollback evidence.
- Production deployment must follow the documented isolated BugLogin release workflow and must not mutate BugIdea or legacy BugLogin services.

## Acceptance outcomes

### Minimum usable desktop app

- A clean app start reaches a stable sign-in or authenticated workspace screen; refresh/restart does not flash the wrong screen or lose the session without an explanation.
- A user can sign in, recover an expired/revoked session safely, select an authorized organization/workspace, and see a useful first-use empty state when it has no profiles.
- In the selected workspace, the user can create, view, edit, clone, group, archive/restore, and delete profiles according to their server-enforced role. Profile data remains after app/API restart.
- A user can use no proxy, add/select/check/edit/remove a proxy, and see a useful failure state. Proxy credentials remain protected and never appear in logs, ordinary API reads, or error toasts.
- Groups and profile assignments persist and respect workspace boundaries. Cookie import/export appears only where implemented and reports validation, partial, and failure outcomes without losing source data.
- The app has usable loading, empty, offline/degraded, forbidden, locked/conflict, and recoverable-error states for the core flow. Errors name the failed action and expose a retry or safe next step.
- Core desktop operations use the canonical Rust/API contracts and PostgreSQL-backed data path. A local-only cache or mock cannot be presented as synced/current data.
- A Windows desktop smoke journey covers startup → sign-in → workspace → create/edit profile → assign group/proxy → restart → verify persisted data. macOS evidence is still required before claiming a cross-platform release.

#### MVP basics that must be present in the desktop flow

- The profile workspace has a real profile table with search, group filtering, useful empty/loading/error states, and stable refresh after create/edit/delete. Profile name, tags, notes, group, proxy, browser engine/version, and cookie data must round-trip through the canonical API where each field is supported.
- Create and edit expose the same core choices: name, group, browser engine and version, proxy (including an explicit no-proxy choice), and supported profile settings. A saved profile reopens with its exact saved choices; defaults must not silently replace them.
- Group basics include list/create/rename/delete and profile assignment, with workspace-scoped authorization and clear behavior when a group is in use.
- Cookie import accepts the supported input format, validates before mutation, reports rejected rows, and preserves the source/profile on failure. Cookie export is available only through an authorized operation and never exposes data in logs or routine diagnostics.
- Proxy basics include no proxy, custom and saved proxy selection, supported protocol/host/port/optional credentials, connectivity check, edit/remove, and secret-safe structured results. Saving or assigning a proxy must not discard credentials.
- First-run, session expiry, offline API, forbidden access, and restart recovery are explicit states with a safe next action. No visible core control may be a dead button or rely on mock/local-only data presented as server state.

These are the product basics for the first usable desktop release. They do not add a second UI system: desktop presentation must follow the pinned Donut reference, `docs/design/ui-authority.md`, the information-architecture row, and screen-state matrix.

These are existing capabilities, not permission to add parallel DTOs, local persistence authorities, or new UI styling. Reuse `CAP-DESK-001`, `CAP-ID-001/002`, `CAP-ORG-001`, `CAP-WS-001`, `CAP-PROF-001/002/003`, `CAP-NET-001/002`, `CAP-COOK-001`, the canonical command registry, and the Donut UI authority.

### Browser runtime and launch

- The create-profile flow can obtain verified runtime versions and clearly shows installed, available-to-download, downloading, failed, and ready states.
- Opening a profile with a missing runtime triggers the supported download/install path (or offers an explicit install action and resumes launch); it never launches an unverified binary.
- A cold-install desktop test downloads the exact selected version, verifies the catalog signature and artifact SHA-256, installs the full browser package, persists and revalidates its receipt after app restart, and opens a real profile window.
- The same REST catalog contract and desktop installer flow serve local PostgreSQL development, staging, and production. Environment-specific catalog URLs/keys/artifact origins remain separate.
- The signed runtime catalog is available in local and staging API deployments. Production readiness reports catalog availability only after startup verification succeeds.

### BugLogin application update

- Update manifests are BugLogin-owned, signed, channel/platform/version scoped, and distributed independently of browser runtime versions.
- The desktop checks, downloads, verifies, stages, applies, reports failures, and can recover/rollback without corrupting profile data or leaving an unlaunchable installation.
- Update behavior is proven from an older built app to a newer built app on supported release platforms; a mocked manifest alone is insufficient.

### Accounts, workspace, sync, and locks

- Authentication uses canonical API endpoints, durable PostgreSQL state, expiring/rotating sessions, revocation, and clear invalid-session recovery.
- Account, organization, workspace, membership, and role relationships are enforced by the API on every operation and are isolated across tenants.
- Two authenticated devices can sync profile metadata, proxy assignment, and encrypted profile payloads with integrity verification, idempotent retry, conflict handling, device revocation, and restart persistence.
- Team locks use server-authorized leases with expiry, renewal/heartbeat, fencing against stale writers, and recovery after device/network loss.

### Extensions, Groups, and MCP

- Each visible Extensions and Groups operation has a canonical API contract, API authorization, durable behavior, audit/error states, and desktop wiring.
- MCP tools are generated/registered from the same command contracts, validate inputs, enforce the same API permissions, redact secrets, and have execution-level evidence for the in-scope operations.
- Contract parity is exact between source API, generated clients/command registry, and deployed staging API before release.

### Production release

- Windows and macOS packaged builds are signed/notarized as required, have provenance and SBOM, and use production-only API/update/runtime trust configuration.
- Staging rehearses DB migration, runtime publication, app update, authentication, sync, locks, Extensions/Groups/MCP, failure recovery, and rollback with synthetic accounts/data.
- Production deployment occurs only after readiness, OpenAPI parity, backup/restore, security, and rollback gates pass. Post-deploy authenticated smoke checks prove the app can log in, create/open a profile, and sync across two devices.

## Explicitly out of scope for this MVP

- Mobile browser runtimes or OS fingerprint options without a supported executable engine and release evidence.
- Treating legacy Donut-installed binaries or old application versions as BugLogin production artifacts.
- Data migration from legacy production unless a separate owner-approved migration plan and rehearsed rollback are complete.
- Declaring production ready based only on compiling, unit tests, a reachable website, or a catalog JSON file that has not been consumed by a packaged desktop app.

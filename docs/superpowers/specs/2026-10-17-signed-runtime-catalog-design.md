# Signed Runtime Catalog Design

## Scope

Verify runtime catalog entries with a concrete Ed25519 trust verifier and a stable signed payload. The API may publish a catalog only after validating every entry with configured Ed25519 public keys. This does not provision production trust keys or wire a desktop updater.

## Signature contract

- Catalog schema version `1` remains supported for raw single-file releases and retains its existing signature payload.
- Catalog schema version `2` adds signed `packageFormat` (`raw`, `zip`, or `dmg`) and optional signed `archivePrefix`. A missing `archivePrefix` means archive paths are preserved as-is.
- Release signatures are 64-byte Ed25519 signatures encoded as 128 hexadecimal characters.
- A verifier receives an explicit map of non-empty key IDs to 32-byte public keys. Missing/unknown keys, malformed signatures, and invalid signatures fail closed.
- A published catalog must contain at least one supported release; empty catalogs are rejected.
- The signed message begins with the bytes `buglogin.runtime-release.v1\0`.
- Append, in order, the unsigned 64-bit big-endian byte length and UTF-8 bytes for runtime label, version, platform label, architecture label, relative path, HTTPS source, lowercase SHA-256, and key ID. Append `sizeBytes` as unsigned 64-bit big-endian last.
- Schema `2` signatures begin with `buglogin.runtime-release.v2\0` and append the UTF-8 `packageFormat` and `archivePrefix` (empty when absent) after `relativePath`, before `source`; all remaining v1 fields retain their order.
- Labels are lowercase contract names (`wayfern`, `bugmini`, `windows`, `macos`, `linux`, `x64`, `arm64`); labels for other enum variants are explicit in Rust and do not depend on `Debug` output.

## Trust and install boundary

- Production public keys must be pinned or delivered by a separately authenticated administrative channel; a key inside the catalog cannot establish trust in itself.
- The API reads bounded catalog and trust-key files at startup, rejects symlinks, unknown fields, and duplicate key IDs, and serves the verified catalog through a public read-only endpoint with ETag caching. Missing configuration makes the endpoint unavailable; unsigned fallback is forbidden.
- Production API readiness returns `503 runtime_catalog_unavailable` until the signed catalog is configured and verified. Local API development remains ready without a catalog so auth/profile CRUD can be exercised while runtime installation stays unavailable.
- A release source must parse as an absolute HTTPS URL with a host and no user information or fragment. Query parameters remain allowed for signed artifact URLs.
- Signed runtime packages are capped at 4 GiB; ZIP expansion is capped at 3 GiB and 100,000 entries to bound disk use on desktop machines.
- Artifact bytes must be streamed to a bounded staging file, then size- and SHA-256-verified before atomic installation. Failed verification preserves the installed runtime.
- ZIP packages must reject path traversal, symlinks, duplicate/conflicting paths, excessive file counts, and excessive expanded size. `archivePrefix`, when present, is stripped from each entry; `relativePath` names the executable inside the extracted runtime directory. Install under `<runtime>/<platform>/<architecture>` so each platform has one active runtime and an atomic directory swap can retain the previous directory for rollback. Extract into an invisible same-filesystem staging directory, confirm the signed `relativePath` executable exists inside it, then atomically promote the complete runtime directory.
- DMG packages are accepted only for signed macOS Wayfern releases in catalog schema version `2`. Download and verify the signed size and SHA-256 before mounting read-only with the native `hdiutil` tool; require the signed executable path to be inside a top-level `.app` bundle, copy that bundle with `ditto` into an invisible same-filesystem staging directory, detach successfully, verify the executable is a regular file, then atomically promote the complete runtime directory. Bound `hdiutil` and `ditto` execution time and attempt forced detach after any failed mount/copy/detach path. Do not mount unverified bytes or use shell interpolation.
- Desktop/API integration, key rotation/revocation, and real release artifact evidence are separate gates.

## Desktop retrieval boundary

- Desktop Rust fetches `GET /v1/runtime/catalog` without a bearer token, sends the required correlation ID, caps the streamed response at 2 MiB, and verifies every release before returning the catalog across the Tauri boundary.
- The verifier trusts only BugLogin runtime keys pinned at desktop build time with `BUGLOGIN_RUNTIME_TRUST_KEYS_JSON` (key ID to 32-byte public-key hex map). The legacy single-key variables remain a fallback. Missing, malformed, or empty pin configuration fails closed. A response-provided key is never trusted.
- Catalog retrieval does not authorize profile launch or installation. The UI must continue to show the runtime unavailable until the platform-specific signed release is installed and the verified executable is registered with the process controller.
- When the catalog is unavailable, profile metadata CRUD may still proceed without `runtimeVersion`; the create flow must label this as profile-settings creation and state that install/launch remains unavailable. Production runtime launch stays fail-closed until the signed release is installed. A catalog outage must not trap the user in onboarding or masquerade as a ready browser.
- Explicit `BUGLOGIN_WAYFERN_*` and `BUGLOGIN_BUGMINI_*` executable overrides are permitted only in debug builds using local authority or an explicit loopback HTTP API. Release builds and debug builds targeting a hosted API reject these overrides; a self-supplied SHA-256 is integrity checking, not runtime provenance.

## Local API development isolation

- A debug desktop configured with an explicit loopback HTTP API uses the production REST/auth/sync path, but must isolate its device identity and default profile cache from the hosted production API.
- `pnpm --filter @buglogin/desktop tauri:dev:local-api` must fail closed unless `127.0.0.1:12344/v1/readyz` identifies a ready BugLogin API, then launch the same production REST/auth/sync path against that loopback API.
- The local launcher clears inherited API bearer token, production data-root, and profile-cache overrides before starting Tauri. It must never fall back to a hosted endpoint.
- The local API receives a separate stable device identity namespace. Its auth session and per-device encryption keys consequently cannot overwrite the hosted API session or key records on the same workstation.
- The default profile cache is `BugLogin-LocalApi/profiles`; an explicit `BUGLOGIN_PROFILE_CACHE_ROOT` remains an operator override.
- Packaged builds continue to reject loopback HTTP endpoints. This development exception must not alter production API host pinning or production storage paths.

## Key rotation and revocation

- Planned rotation: ship a desktop build that pins both old and new public keys; configure the API verifier with both; publish the next catalog signed by the old key and include new-key-signed releases; after the overlap window, publish catalogs signed by the new key; then remove the old key from API and subsequent desktop builds.
- A compromised key requires an emergency desktop build that removes it from the pinned set and an API config update that rejects it. Previously shipped clients that still pin the compromised key cannot be remotely revoked and must be upgraded. Do not claim complete revocation until that client-upgrade boundary is acknowledged.

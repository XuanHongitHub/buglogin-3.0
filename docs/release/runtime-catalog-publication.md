# Runtime catalog publication

The API serves `GET /v1/runtime/catalog` only after validating the entire catalog against configured Ed25519 trust keys. The endpoint is public, read-only, and returns a JSON ETag with a 60-second cache lifetime. Matching `If-None-Match` returns `304`.

## API configuration

For local development, configure the compact JSON pair in `.env.local`:

- `BUGLOGIN_RUNTIME_CATALOG_JSON`: a compact `RuntimeCatalog` JSON value, limited to 24 KiB for Windows environment compatibility.
- `BUGLOGIN_RUNTIME_TRUST_KEYS_JSON`: a JSON object mapping key IDs to 32-byte Ed25519 public keys in hex. This same public map is embedded in the local desktop dev build.

The API also retains file-based configuration for hosted deployments and older local setups. Configure one catalog source and one matching trust-key source:

- `BUGLOGIN_RUNTIME_CATALOG_PATH`: absolute path to a JSON `RuntimeCatalog` file, maximum 2 MiB.
- `BUGLOGIN_RUNTIME_TRUST_KEYS_PATH`: absolute path to a JSON array of `{ "keyId": "...", "publicKey": "64 lowercase or uppercase hex characters" }`, maximum 64 KiB.

Do not mix inline JSON and file-path sources for either item. The catalog and trust-key values are public verification data, not signing secrets. Keep the private signing key offline or in the protected release signer.

For local PostgreSQL development, `scripts/dev-local-api.mjs` forwards either the inline pair or the path pair from `.env.local` or the process environment into the API process. A partial pair or mixed configuration fails before the API starts. If neither source is set, local API readiness remains available for account/profile metadata work and `/v1/runtime/catalog` correctly reports `503 runtime_catalog_unavailable`.

For local desktop development, `scripts/tauri-dev-local-api.mjs` forwards `BUGLOGIN_RUNTIME_TRUST_KEYS_JSON` from `.env.local` or the process environment into the Tauri build. The local API converts that same object to its catalog trust-key contract. Never put private signing material in `.env.local` or the repository.

The service rejects symlinks, non-files, oversized input, duplicate key IDs, unknown JSON fields, empty catalogs, unsupported runtimes, invalid release metadata, unknown key IDs, and invalid signatures. Configuration is loaded and verified before the HTTP listener starts. If neither source is configured, the route returns retryable `503 runtime_catalog_unavailable`; it never serves an unsigned fallback.

The runtime catalog gates browser installation and launch. Local API development may remain ready without a catalog so auth and metadata CRUD can be exercised, but production readiness requires a configured, verified signed catalog. In production, `/v1/readyz` returns retryable `503 runtime_catalog_unavailable` until that condition is met. Successful readiness includes `X-BugLogin-Runtime-Catalog: available`; the response body remains unchanged. Runtime download and launch still fail closed until a verified artifact is installed on the desktop.

## Signing and rotation

`scripts/runtime/sign-catalog.mjs` creates a schema-v2 catalog entry from an already approved local artifact. It hashes the artifact as a stream, signs the Rust-compatible Ed25519 payload, writes the catalog and public trust-key JSON to new files without overwriting existing files, and can preserve other verified schema-v2 releases using `--existing-catalog` and `--existing-trust-keys`. It accepts a PEM Ed25519 private key by file path; never place the private key in the repository, command arguments, environment, or API configuration. The script does not download or establish provenance for the browser artifact: release operators must verify the source, version, license, platform, and executable path before signing. Use `node --test tests/release/runtime-catalog-signing.test.mjs` for the synthetic signer checks.

Keep signing private keys offline or in the approved release-signing service. The API configuration contains public keys only. To rotate a key, add the new public key alongside the existing key, sign new catalog entries with the new key ID, publish the catalog, then remove the old key only after supported desktop versions have moved to the new trust set. Revocation of a compromised key requires removing it from the trust file and restarting the API; this immediately makes catalogs containing signatures from that key fail closed.

The API endpoint validates the server-side catalog but does not establish a trust root for desktop clients. Desktop public-key pinning, catalog retrieval, platform selection, install, rollback, and real signed artifact verification remain separate release work. Do not claim runtime auto-update complete until those client and release gates pass.

Desktop source now pins the catalog trust root at build time with `BUGLOGIN_RUNTIME_TRUST_KEY_ID` / `BUGLOGIN_RUNTIME_PUBLIC_KEY_HEX` or the key-rotation map `BUGLOGIN_RUNTIME_TRUST_KEYS_JSON`. Its Rust client makes an unauthenticated, correlation-scoped GET and verifies every catalog entry before the Tauri command returns it. Missing/malformed build-time pins fail closed. The active profile-creation and first-run flows now consume the host-filtered catalog; installation runs through the Rust bounded HTTPS installer, persists a signed release receipt, restores the target after restart, and refreshes the process allowlist. This wiring is source-only and has not passed the consolidated build/test pass. The trust inputs are not provisioned and there is still no signed BugLogin catalog or verified distributable artifact.

## Workstation readiness check — 2026-10-17

The current workstation has none of the runtime catalog/trust-key environment settings configured. The only browser archives found in the local source/reference roots are Cloak archives and tiny reference test archives; they are not signed Wayfern or Bugmini releases and must not be published or installed as a substitute. Desktop catalog/install integration was added to the active app source on 2026-10-18, but it has not been built and there is no usable signed runtime artifact for an end-to-end installation.

The isolated production API host was checked over the authorized SSH alias. Neither `BUGLOGIN_RUNTIME_CATALOG_PATH` nor `BUGLOGIN_RUNTIME_TRUST_KEYS_PATH` is configured in `/etc/buglogin3/api.env`; the check printed variable names only and read no secret values. The API therefore cannot publish a runtime catalog in the current deployment. The desktop catalog/install path was subsequently connected in source (2026-10-18), but that does not change the missing server configuration or provide an artifact.

## Upstream artifact shape — 2026-10-17

The pinned Donut runtime discovery flow reads `https://donutbrowser.com/wayfern.json`. A current read-only request returned Wayfern `154.0.8037.57`; Windows x64 points to a ZIP whose CDN `Content-Length` is `1,683,221,729` bytes. The JSON exposes version and download URLs, not an artifact SHA-256. The CDN returned a multipart ETag (`91bcf124f4441465060137f83b238030-201`), which is not the catalog's required SHA-256; `https://download.wayfern.com/version.json` returned 404.

On 2026-10-20, the official Windows x64 ZIP was downloaded to the non-repository staging path `E:\buglogin-builds\runtime\wayfern-154.0.8037.57_windows_x64.zip`. Its byte length is `1,683,221,729` and locally computed SHA-256 is `675daa21195d97a6f1ab5c36dac61b16af019707356fa9df41a7bbdab37457da`. ZIP inspection found 2,262 entries (about 2.52 GiB uncompressed), one top-level directory, no traversal/symlink entries, and `chrome.exe` at `wayfern-154.0.8037.57_windows_x64/chrome.exe`. This establishes package integrity and executable layout for the local candidate; it is not yet represented by a signed catalog or proven in the app.

The Rust catalog now represents schema-v2 `raw`/`zip` packages and signs `packageFormat` plus optional `archivePrefix`, while schema-v1 raw signatures retain their original payload. The ZIP installer source stages and hashes the archive, bounds it to 4 GiB compressed / 3 GiB expanded / 100,000 entries, rejects unsafe or conflicting paths and symlinks, confirms the signed executable path, and swaps the complete platform runtime directory with rollback. Desktop wiring was added on 2026-10-18, but neither installer nor client integration has passed the consolidated build/test run. The downloaded Wayfern package has a locally computed SHA-256, but no signed BugLogin catalog or production signing-key custody is configured; do not use the multipart ETag as a substitute checksum.

## Live endpoint recheck — 2026-10-17

Read-only requests confirmed `api3.buglogin.com/v1/readyz` returns `200` and `api3.buglogin.com/v1/openapi.yaml` returns `200`, but the served OpenAPI document still names `https://api.buglogin.com` as its server, has 94 paths, and does not include `/v1/runtime/catalog`. A GET to `/v1/runtime/catalog` with the required correlation header returns `404`. The production route therefore does not yet serve the locally implemented signed catalog endpoint. `app3.buglogin.com/login` and `/download` both return `200`; that proves page reachability, not that a desktop release is published or usable.

## Current status — 2026-10-20

The older workstation notes above are historical. Local development is now configured with `E:\buglogin-builds\runtime\wayfern-catalog.json` and the API-format public trust-key list at `E:\buglogin-builds\runtime\wayfern-trust-keys-api.json`. The embedded Wayfern 154.0.8037.57 Windows x64 entry verifies against the desktop-pinned Ed25519 key; the API trust file is a JSON array while the desktop embedded trust file is a JSON map. The current source API was restarted through `pnpm --dir apps/desktop api:dev:local`; `/v1/runtime/catalog` returns `200`, readiness returns `200` with `X-BugLogin-Runtime-Catalog: available`, and persistence reports PostgreSQL. The Tauri dev launcher now runs the current desktop binary with Vite HMR from `E:\buglogin-target`.

This closes the local catalog-fetch configuration gap only. The actual browser install/profile-launch flow has not yet been verified. Production still returns `404` for `/v1/runtime/catalog`; the verified catalog has only Windows x64. The current first-party `https://donutbrowser.com/wayfern.json` advertises Wayfern 154.0.8037.57 for Windows x64, Linux x64 and macOS arm64; macOS x64 and Windows/Linux ARM64 are unavailable. The macOS arm64 DMG is about 1.55 GB and its multipart ETag is not a SHA-256. It must be downloaded, hashed, provenance checked and signed through approved key custody before adding it to a trusted catalog. Neither Windows/macOS production artifacts nor release readiness are established by this local setup.

# BugLogin Native Desktop Updater

## Status

Approved for a controlled implementation wave under Gate 0. This is not release approval.

## Context

The desktop has legacy updater commands that return a hard-coded version, claim an update is ready without downloading it, and restart the application without installing an update. The active app does not have a complete signed binary update flow. The repository also has an Ed25519 manifest verifier, but no client that applies updates from that manifest.

## Decision

Use the supported Tauri v2 updater plugin for Windows and macOS binary installation. Use the plugin's mandatory artifact signature and pinned minisign public key for install authorization. Do not treat the repository's Ed25519 release-evidence manifest as an updater trust root; retain it only for bundle/evidence verification until a separate compatibility review removes or supersedes it.

The updater endpoint is HTTPS and serves the Tauri static JSON schema. The release pipeline must assemble one immutable `latest.json` from the signed Windows x64, macOS arm64, and macOS x64 updater artifacts. RC and stable channels must not share mutable metadata. Promotion changes channel metadata to point to already verified artifact URLs and hashes; it does not rebuild artifacts.

BugLogin 3.0 uses the existing DigitalOcean Spaces bucket `mavelnut-login` without modifying the 1.0 feed. Store immutable 3.0 updater artifacts under `releases/desktop/v3/artifacts/v<version>/<run-id>/`; publish RC and stable Tauri feeds separately at `releases/desktop/v3/rc/latest.json` and `releases/desktop/v3/stable/latest.json`. Stable updater builds pin the stable HTTPS endpoint. The 1.0 `releases/desktop/latest.json` is a download catalog with null signatures/checksums and is not compatible with the Tauri updater schema.

The update key pair is a release credential. The private key is only a protected GitHub environment secret, never generated or stored by application code. The public key is injected into the release build as a Tauri config overlay and is safe to publish. Local development builds have no update endpoint and must report updater unavailable without failing startup.

### Windows asset embedding

Tauri `build.frontendDist` must remain a relative directory path. On Windows, passing an absolute drive path such as `G:\\buglogin-target\\desktop-dist` is parsed as a URL with scheme `G:`; the packaged app then opens the build directory instead of embedding and serving `index.html`. The frontend build output stays under `CARGO_TARGET_DIR`; the Tauri build wrapper stages it temporarily at `apps/desktop/dist`, passes `../dist`, and removes that staging directory after packaging.

## Behavior

- Check reports the current version and an update only when a configured signed endpoint returns a valid newer artifact.
- Download/install uses the plugin's signature verification and OS-native install flow. The UI must show progress, failure, and restart state; it cannot claim readiness before signature verification/install preparation succeeds.
- No unsigned fallback, downgrade, arbitrary URL, or API-controlled binary execution is allowed.
- Missing feed/key, unsupported platform, network failure, or invalid signature is returned as a structured error and leaves the installed app unchanged.
- Release metadata is platform/architecture specific and immutable per version. Stable promotion only references the exact RC artifacts that passed Windows/macOS update E2E.

## Security and release gates

- Protected release environment owns the updater private key; only signing jobs can read it.
- Public key pin is embedded in each signed release. Key rotation requires a dual-key transition release and a tested revocation/rollback procedure.
- The S3 stable `latest.json`, GitHub release assets, installer signatures, artifact checksums, SBOM, and provenance must refer to the same immutable version/artifacts.
- Windows/macOS clean install and previous-version-to-current-version update are required before stable promotion.
- Until the updater key, Windows Authenticode credentials, Apple notarization credentials, and protected release environment are available, code may be validated but no production-ready release may be claimed.

## Acceptance

- Legacy hard-coded updater success responses are removed.
- Native plugin configuration has no production endpoint or trust key in development defaults.
- Release build injects HTTPS feed and pinned public key; generated updater artifacts carry Tauri signatures.
- A manifest assembler rejects missing, duplicate, mismatched, or unsigned platform artifacts.
- Windows and both supported macOS architectures install a signed update from RC, restart on the new version, and can recover through channel rollback without replacing immutable artifacts.

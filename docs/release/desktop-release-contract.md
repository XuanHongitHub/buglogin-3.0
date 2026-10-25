# Desktop Release Contract

This file is the shared desktop release contract. The canonical copy lives in
`keyduc91/Malvanut-Login`; `XuanHongitHub/buglogin-3.0` must carry the same file
before a release workflow is run. If the copies differ, stop and synchronize
them before building or publishing.

## One source and one immutable build

- The product repository owns app source, version, and this release contract.
- The Actions repository owns signing/publishing workflow configuration and
  must build the exact reviewed product commit. Never mix source and workflow
  snapshots from different commits without reviewing their release-critical
  differences.
- A release version is used once. Build, sign, notarize, hash, and attest from
  one immutable commit/tag. Never replace bytes or manifests for an existing
  version; a correction gets a new version and tag.
- Match the root Cargo workspace version, desktop package version, Cargo.lock,
  Tauri config, tag/source SHA, artifact names, and updater/download metadata.

## Embed the UI in the application

- `CARGO_TARGET_DIR` selects build/cache output only. Never pass its absolute
  path (especially a Windows drive path such as `G:\buglogin-target\desktop-dist`)
  as Tauri `build.frontendDist`.
- Tauri can interpret `G:\...` as a URI with scheme `G:`. That produces a
  signed app which opens a directory listing or depends on a build folder from
  the builder's machine instead of containing the BugLogin UI.
- Build frontend assets into the configured target, copy them to a unique,
  temporary directory inside `apps/desktop`, and pass only its relative path to
  Tauri. Clean the temporary directory after success, failure, or cancellation.
- CI must test the wrapper-generated merged config and reject drive-absolute
  frontend paths. A successful compile, signature, or version label does not
  prove the UI is embedded.
- Before publication, install the signed Windows package on a clean device (or
  clean VM) with the build target unavailable, launch the installed app, and
  verify the real BugLogin UI loads. Record screenshot/version and test result.
  Do the equivalent launch check on macOS.

## Update behavior and feed

- “Check for updates” is check-only: it may fetch and validate metadata, then
  show the version and explicit “Update now” / “Later” choices. It must not
  download, install, close, or restart the app by itself.
- Manual-check results must not enter the state/effect used by background
  auto-download. Keep the manual and automatic update paths distinguishable;
  test that pressing “Check for updates” does not invoke the install command
  until “Update now” is selected. Do not copy an auto-download-on-detection
  pattern into a manual-check action.
- Automatic startup update is a separate user setting and must respect the
  persisted preference. Any prepared update requires an explicit restart
  action. Close/stop the app and managed daemon cleanly before replacing files;
  relaunch and verify existing profile data/settings afterward.
- Publish updater metadata only to the configured public S3/Spaces feed. GitHub
  Actions artifacts/releases are build evidence or download mirrors; they are
  not the updater source unless the app is explicitly configured to use them.
- Before changing the stable feed, validate the exact feed URL embedded in the
  app: HTTP GET 200, JSON schema/version/platform keys, public updater key,
  non-empty platform signatures, HTTPS immutable artifact URLs, and matching
  SHA-256/checksum evidence. HEAD/download every referenced artifact and verify
  bytes against the manifest. Read the uploaded manifest back from public S3.
- Only publish manifests after the installed-app UI and update smoke tests pass.
  If publication is already live and a defect is found, stop promotion, publish
  a new version, and point stable metadata to the newly verified immutable
  artifacts; never mutate a version's existing artifacts.

## Signing and evidence

- Windows and macOS Tauri updater signatures must be cryptographically verified
  against the configured public updater key before their artifacts are uploaded.
  A neighboring `.sig` file is not signature evidence by itself. macOS packages
  also require Developer ID signing, notarization acceptance, stapling, and
  Gatekeeper validation.
- Keep private signing keys, Apple credentials, S3 credentials, and tokens in
  protected CI secrets only. Never print them or copy them into source/handoff.
- Preserve SHA-256 checksums, SBOM, provenance/attestation, signing and
  notarization results, manifest readback, and OS install/update smoke evidence
  under the release's immutable evidence record.
- Missing Windows or macOS installed-app evidence means the release is not
  production-ready. Do not infer install/update success from a green build or
  a successful S3 upload.

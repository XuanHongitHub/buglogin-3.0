# Stable-only desktop release design

## Goal

Publish BugLogin 3.0.6 as one official stable release without a public RC feed or GitHub prerelease. Keep the existing Windows/macOS signing, notarization, artifact verification, immutable storage, and device-acceptance gates.

## Release state flow

1. A manually dispatched, tag-bound workflow builds and signs Windows x64 and macOS arm64. It stores the exact candidate bundle only as a time-limited GitHub Actions artifact; it creates no GitHub Release and writes no public RC manifest/object.
2. The exact candidate artifacts are installed and acceptance-tested on supported Windows and macOS devices. Evidence records tag, source SHA, workflow run ID, artifact hashes, OS/build, results, and reviewer.
3. A separate protected stable-publish workflow accepts the completed candidate run ID and HTTPS evidence URL. It verifies source/tag/run binding, artifact checksums, signatures, SBOM/provenance, and the two manifests before publishing immutable artifacts and stable manifests to S3. It creates a normal GitHub stable release, never a prerelease.
4. Post-publish checks verify public GET/HEAD, updater signature, landing redirects, and clean install/update. Failed checks stop the workflow and use the documented stable-manifest rollback path.

## Manifest contract

- `latest.json` is Tauri updater metadata only: `version` and signed `platforms` keys `windows-x86_64` and `darwin-aarch64`.
- `downloads.json` is the landing/manual-installer manifest only: `version` and `platforms.windows.url` / `platforms.macos.url`.
- Both manifests reference the same version and immutable artifact prefix `releases/desktop/v3/artifacts/<tag>/<candidate-run-id>/`.
- Stable objects are `releases/desktop/v3/stable/latest.json` and `releases/desktop/v3/stable/downloads.json`. No `rc/` object is written by this release flow.

## Gates and constraints

- The workflow must build from an annotated final-semver tag reachable from `main` in `XuanHongitHub/buglogin-3.0`; that tag must contain the reviewed source snapshot from `keyduc91/Malvanut-Login`.
- No stable publish is possible without successful signed Windows and macOS artifacts and reviewed HTTPS device-acceptance evidence.
- S3 secrets remain in GitHub's protected `release-publish` / `stable-promotion` environments. Secret values never enter logs or artifacts.
- The old RC-producing workflow is removed or made impossible to dispatch after the stable-only flow is enabled.
- API/VPS and landing deployment remain separate operations; desktop publication never silently deploys them or changes DNS.

## Acceptance criteria

- Candidate workflow has no public prerelease creation, S3 write, or RC feed update.
- Stable workflow rejects a run from another workflow, a mismatched tag/source SHA, failed/cancelled run, missing evidence, version mismatch, invalid signature, checksum mismatch, or missing supported platform.
- Stable workflow publishes S3 artifacts before manifests, verifies every URL by public HEAD, and publishes the stable JSON last.
- GitHub release is created as non-draft, non-prerelease, latest, with the same verified assets and checksums.
- Landing route consumes only `downloads.json`; desktop updater consumes only `latest.json`.
- Windows and macOS post-publish install/update smoke and S3 readback are recorded; otherwise release status remains blocked.

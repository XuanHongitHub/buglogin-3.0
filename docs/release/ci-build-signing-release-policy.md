# CI, Build, Signing, Promotion, and Rollback Policy

For the Windows workstation paths and the repository's current GitHub Actions/S3 procedure, see [Windows release workstation runbook](windows-release-workstation.md).
The shared, binding desktop build/update/publish gates are in the [desktop release contract](desktop-release-contract.md); synchronize that file unchanged to the Actions repository before a release.

## Version authority

One release version must match the root `Cargo.toml` workspace version, `apps/desktop/package.json`, `Cargo.lock`, `apps/desktop/src-tauri/tauri.conf.json`, tag or immutable source SHA, artifact names, update manifest, and release notes. Any mismatch stops release.

## Tauri frontend assets and update behavior

- `CARGO_TARGET_DIR` is only a compiler/cache and build-output location. Never pass its absolute Windows drive path (for example `G:\\buglogin-target\\desktop-dist`) as Tauri `build.frontendDist`.
- Tauri `frontendDist` accepts either a URL or a directory. A Windows drive path begins with a URI scheme such as `G:`, so it can be interpreted as an external URL instead of bundled assets. The app can then open the build directory at runtime and show an `Index of ...` listing; another machine may have no such path at all.
- Build frontend files into the selected target, then stage/copy them under a generated, repository-relative directory for the Tauri build. Configure `frontendDist` with that relative directory so Tauri embeds the files. Remove the generated staging directory after packaging.
- The release verifier must reject a drive-absolute `frontendDist` and test the wrapper that constructs Tauri's merged config. The signed Windows install smoke must launch the installed app and confirm it renders the BugLogin UI without reading any workstation build path.
- A manual “Check for updates” action only checks and presents the available version. It must not download, install, close, or restart the app until the user chooses “Update now”; after preparation, present a separate restart action. Automatic startup updates are a separate path and must follow the user's auto-update setting.
- Check the published feed from the exact URL embedded in the app; successful S3 upload alone is not evidence. Verify public GET/JSON schema, platform signatures, immutable asset HEAD/download/checksums, then test update from the previous signed installer.

## CI/release matrix

| Stage | Trigger | Required lanes | Output | Gate |
|---|---|---|---|---|
| Validate | Pull request | Windows, macOS, Linux static/unit where supported | Test reports | Lint, typecheck, Rust fmt/clippy/unit, schema tests |
| Integration | Pull request/main | Windows + macOS runtime-sensitive; service E2E | Integration evidence | Proxy/VPN/sync/API/OpenAPI |
| Migration | Migration/schema change | Windows + macOS | Fixture/golden reports | Required fixtures pass |
| Build | Immutable commit/tag | Windows x86_64; macOS arm64; Intel if supported | Unsigned bundles, SBOM, provenance | Clean/reproducible inputs |
| Sign | Protected job | macOS Developer ID signer; Tauri updater signer for supported platforms | Notarized macOS app; updater-signed archives | Signature and notarization verification |
| Notarize | macOS protected job | macOS | Stapled DMG/app | Apple acceptance |
| Device acceptance | Protected candidate run | Full supported Windows/macOS | Install/update evidence | Exact signed bytes pass |
| Publish stable | Approved successful candidate run | Stable S3 metadata | Immutable assets + manifests | Same hashes as accepted candidate |
| Post-release | Stable publish | Install/update smoke | Canary report | No stop condition |

## Toolchain baseline

Current snapshots provide evidence, not final pins:

- Legacy `1.1.84`: Tauri 2; macOS 14 workflow; Node 22; pnpm 10.32.1; Rust stable.
- DonutBrowser `0.34.0`: pnpm 12.8.1 and upstream multi-platform release patterns. These are reference evidence, not BugLogin pins.

Rebuild must pin one Node, pnpm, Rust, and runner-image version before implementation. Release CI must not float on `stable`.

## Artifact matrix

| Platform | Architecture | Primary | Secondary | Signing |
|---|---|---|---|---|
| Windows | x86_64 | NSIS `.exe` (manual download/install) | The same NSIS `.exe` with Tauri `.exe.sig` when in-app updates are enabled | Authenticode is optional; the Tauri updater signature is required when updater is enabled |
| macOS | arm64 | `.dmg` containing `.app` | Signed updater `.app.tar.gz` | Developer ID, hardened runtime, notarization, stapling |
| macOS | x86_64 | `.dmg` containing `.app` | Signed updater `.app.tar.gz` | Same, if Intel supported |

Linux artifacts need a separate support decision before publication.

Every release includes SHA-256, SBOM, provenance/attestation, license notices, version/feature manifest, migration compatibility, minimum OS, known issues, rollback compatibility.

## Signing controls

- Protected tags/manual approvals only.
- Signing secrets use protected environments and least privilege.
- macOS signing accepts the explicit `APPLE_CERTIFICATE_P12_BASE64` / `APPLE_APP_PASSWORD` names or the already provisioned repository aliases `APPLE_CERTIFICATE` / `APPLE_PASSWORD`; it derives the Developer ID Application identity from the imported keychain when no identity secret is set, and checks the configured Team ID.
- Logs never print certificates, keys, passwords, tokens, or decoded secrets.
- Temporary keychains/certificates removed in `always` cleanup.
- Windows Authenticode certificate and timestamp are optional for the manual-installer release path. Do not block a downloadable Windows installer on them.
- macOS identity/team/entitlements/hardened runtime/notarization/stapling verify.
- Update artifacts are signed separately from installers when the updater capability is enabled. The native Tauri updater client/feed is now implemented in source, but signing credentials and installed-app platform evidence remain outstanding. The signed installer verifier still validates the custom Ed25519 release-evidence manifest when one is present. Stable promotion remains gated on key custody and rollback evidence below.
- The native desktop binary updater uses Tauri updater artifact signatures (minisign format) and a static `latest.json` feed with platform keys `windows-x86_64` and `darwin-aarch64`. Tauri v2 uses the signed NSIS `.exe` directly for Windows updates and `.app.tar.gz` for macOS. The protected `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are consumed only by signing jobs; `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY` is embedded into the build and is safe to publish. The existing Ed25519 `updater-manifest.json` verifier is release evidence only and is not accepted as the Tauri install trust root.
- Signed candidate artifacts are private workflow outputs until device acceptance completes. The stable publisher consumes the successful candidate run, rechecks source SHA, signatures, checksums, SBOM/provenance, platform manifests, and QA evidence, then publishes those exact immutable assets and stable S3 manifests without rebuilding. GitHub is not the updater JSON source; the app reads the configured S3 feed.
- When an Ed25519 release-evidence manifest is included, its verifier must pin the public key from protected release configuration; a public key embedded only in the detached signature envelope is not a trust root. The signing workflow reads the non-secret `BUGLOGIN_UPDATER_PUBLIC_KEY_BASE64` repository variable and fails closed if the manifest's key is absent or mismatched. This evidence key is separate from the compiled Tauri artifact-signing key described below.
- Key rotation/revocation tested before GA.

## Supply chain

- Protected workflow actions pinned to immutable revisions where practical.
- Lockfiles mandatory; frozen installs.
- Clean checkout; generated/untracked product files fail.
- Runtime/sidecar assets pinned with checksums.
- Provenance binds commit, toolchains, workflow, locks, artifact hashes.
- Registry immutable; no same-version replacement.

## Channels

| Channel | Audience | Compatibility | Signing | Retention |
|---|---|---|---|---|
| `dev` | Developers | Disposable | Local optional | Short |
| `nightly` | Internal testers | No downgrade promise | Signed when distributed | Rolling + incident holds |
| `stable` | Users | Forward migration + rollback docs | Mandatory | Support policy |

Do not publish or maintain an RC feed for the current desktop release flow. Development builds remain separate from the stable updater feed.

## Promotion

1. Build once from the immutable source commit/tag.
2. Sign/notarize and verify, then retain the exact immutable candidate artifacts privately.
3. Install those exact bytes and complete Windows/macOS acceptance, including previous-stable update behavior.
4. Review defects, migration evidence, SBOM/provenance, and residual risks; obtain the required release approval.
5. Publish the same accepted hashes and stable S3 metadata without rebuilding.
6. Read the public feed and every artifact back, then run clean-install and previous-stable update canaries.

Rebuilding during stable publication is prohibited. Any binary change requires a new version and a complete acceptance cycle before stable metadata changes.

## Stop conditions

Version/hash mismatch; dirty/unreproducible inputs; missing signature/timestamp/notarization/stapling/checksum/SBOM/provenance; critical/high data, auth, ACL, runtime, sync, migration, update, or cleanup defect; unsupported migration accepted; missing/flaky required lane; previous-stable upgrade failure; unknown rollback compatibility; suspected key/CI credential exposure.

## Artifact rollback

Rollback changes channel metadata; immutable assets remain for investigation.

| Situation | Action |
|---|---|
| Not broadly installed | Remove stable reference; point to previous known-good |
| Feed served, data unchanged | Repoint feed; publish incident notice |
| Forward-only data migration | Do not auto-downgrade; use tested data recovery path |
| Signing compromise | Revoke key/certificate; disable feed; rotate and publish clean release |
| Bad sidecar only | Roll back sidecar manifest only if contract permits |

Procedure:

1. Freeze promotion and update manifests.
2. Record versions, hashes, channels, exposure times.
3. Repoint stable metadata to previous verified artifact.
4. Preserve affected artifacts/evidence; never overwrite.
5. Verify clean install/update from repointed feed.
6. Apply migration runbook if schema changed.
7. Publish exact user action and compatibility warning.
8. Create new fix version; never reuse withdrawn version.

## Unresolved infrastructure needs

- Exact supported Windows/macOS versions and Intel policy.
- Node, pnpm, Rust, and runner-image pins.
- Apple team, certificate custody, notarization authentication.
- Update signing algorithm/key custody/rotation.
- Protected Tauri updater signing key pair: `TAURI_SIGNING_PRIVATE_KEY`, optional matching passphrase `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and repository variable `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY`.
- Protected `BUGLOGIN_UPDATER_PUBLIC_KEY_BASE64` value for CI manifest verification, plus a separately designed and pinned desktop updater trust configuration.
- Artifact registry, SBOM/provenance format, retention.
- Stable rollback window and database downgrade policy.
- Hosted runners versus dedicated physical E2E machines.
- Protected GitHub environment reviewers for `release-signing`, `release-publish`, and `stable-promotion`; the workflow files cannot enforce these approval lists by themselves.

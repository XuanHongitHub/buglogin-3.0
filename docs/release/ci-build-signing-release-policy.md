# CI, Build, Signing, Promotion, and Rollback Policy

## Version authority

One release version must match `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`, tag, artifact names, update manifest, and release notes. Any mismatch stops release.

## CI/release matrix

| Stage | Trigger | Required lanes | Output | Gate |
|---|---|---|---|---|
| Validate | Pull request | Windows, macOS, Linux static/unit where supported | Test reports | Lint, typecheck, Rust fmt/clippy/unit, schema tests |
| Integration | Pull request/main | Windows + macOS runtime-sensitive; service E2E | Integration evidence | Proxy/VPN/sync/API/OpenAPI |
| Migration | Migration/schema change | Windows + macOS | Fixture/golden reports | Required fixtures pass |
| Build | Immutable commit/tag | Windows x86_64; macOS arm64; Intel if supported | Unsigned bundles, SBOM, provenance | Clean/reproducible inputs |
| Sign | Protected job | Native OS signer | Signed installers/apps | Signature verification |
| Notarize | macOS protected job | macOS | Stapled DMG/app | Apple acceptance |
| E2E RC | Manual/protected | Full supported Windows/macOS | Acceptance evidence | Thresholds pass |
| Publish RC | Approved commit | RC channel | Immutable assets/manifest | Two-person approval |
| Promote stable | Existing RC artifacts | Stable metadata | Stable manifest | Same hashes as tested RC |
| Post-release | Stable publish | Install/update smoke | Canary report | No stop condition |

## Toolchain baseline

Current snapshots provide evidence, not final pins:

- Legacy `1.1.84`: Tauri 2; macOS 14 workflow; Node 22; pnpm 10.32.1; Rust stable.
- DonutBrowser `0.28.2`: pnpm 11.10.0; Tauri CLI 2.11.4; repository Node value 23; upstream multi-platform release patterns.

Rebuild must pin one Node, pnpm, Rust, and runner-image version before implementation. Release CI must not float on `stable`.

## Artifact matrix

| Platform | Architecture | Primary | Secondary | Signing |
|---|---|---|---|---|
| Windows | x86_64 | NSIS `.exe` | Optional portable `.zip` | Authenticode SHA-256 + trusted timestamp |
| macOS | arm64 | `.dmg` containing `.app` | Optional updater bundle | Developer ID, hardened runtime, notarization, stapling |
| macOS | x86_64 | `.dmg` containing `.app` | Optional updater bundle | Same, if Intel supported |

Linux artifacts need a separate support decision before publication.

Every release includes SHA-256, SBOM, provenance/attestation, license notices, version/feature manifest, migration compatibility, minimum OS, known issues, rollback compatibility.

## Signing controls

- Protected tags/manual approvals only.
- Signing secrets use protected environments and least privilege.
- Logs never print certificates, keys, passwords, tokens, or decoded secrets.
- Temporary keychains/certificates removed in `always` cleanup.
- Windows timestamp URL and certificate identity configured; empty values block stable.
- macOS identity/team/entitlements/hardened runtime/notarization/stapling verify.
- Update metadata signed separately from installers.
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
| `rc` | Release/E2E cohort | Stable-upgrade path required | Production-equivalent | Through stable window |
| `stable` | Users | Forward migration + rollback docs | Mandatory | Support policy |

Separate manifests per channel. Less-stable channels require explicit opt-in.

## Promotion

1. Build once from immutable commit/tag.
2. Sign/notarize and verify.
3. Store immutable artifacts by hash.
4. Publish hashes to RC.
5. Run staging and full Windows/macOS acceptance against downloaded RC artifacts.
6. Review defects, migration evidence, SBOM/provenance, residual risks.
7. Obtain QA lead plus release commander approval.
8. Promote exact hashes by publishing stable metadata.
9. Run clean-install and previous-stable update canaries.

Rebuilding for stable is prohibited. Any binary change creates a new RC.

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
- Windows code-signing provider, certificate, timestamp authority.
- Apple team, certificate custody, notarization authentication.
- Update signing algorithm/key custody/rotation.
- Artifact registry, SBOM/provenance format, retention.
- Stable rollback window and database downgrade policy.
- Hosted runners versus dedicated physical E2E machines.

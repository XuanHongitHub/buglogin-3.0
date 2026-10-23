# Release signing secret compatibility plan

Status: in progress

## Phase 1 — align existing Apple secrets with the protected workflow

- [x] Declare the existing repository secret aliases in the reusable signing workflow.
- [x] Prefer explicit new names, falling back to the configured legacy repository names for the certificate and app-specific password.
- [x] Derive and validate the Developer ID Application identity from the imported keychain when no explicit identity secret is configured; enforce the configured Apple Team ID.
- [x] Preserve required checks for all other Apple credentials and retain all signing/notarization/evidence gates.
- [x] Replace the retired `macos-13` Intel runner with the supported `macos-15-intel` runner in release and platform CI, and synchronize the toolchain pin document/policy tests.
- [x] `actionlint` passes for release, signing, and platform workflows; `node --test tests/release/release-workflows.test.mjs` passes 8/8. Do not execute the signing workflow or publish artifacts from this local checkout.

## Residual release prerequisites

- Windows Authenticode credentials are not currently visible on the designated repository.
- BugLogin runtime catalog public trust keys and a matching approved signed catalog are not configured.
- The updater public-key variable and a completed desktop updater remain required by the release workflow.
- This checkout's Git remote does not match the designated repository, its worktree is dirty, and its commit differs from that repository's `main`; no tag, push, or release is permitted from this state.

## 2026-10-21 continuation — restore the dependency-scan gate

- The latest scheduled OSV workflow on `XuanHongitHub/buglogin-3.0` failed at `Upload to code-scanning`; the pinned reusable workflow defaults `upload-sarif` to true. This repository is private and its GitHub plan does not expose code-scanning/ruleset configuration.
- Set `upload-sarif: false` for scheduled and PR scans. OSV scan failure-on-vulnerability and downloadable scan artifacts remain enabled; only the unsupported Security-tab upload is disabled.
- `node --test tests/release/release-workflows.test.mjs` passes 11/11; `actionlint .github/workflows/osv.yml` and `git diff --check` pass.
- PR checks revealed the same SARIF upload assumption in `.github/workflows/pr-checks.yml`; disable that upload there while preserving the OSV scan and vulnerability failure gate. Remove now-unused `security-events: write` from both OSV callers.
- CodeQL also fails during SARIF upload for this private repository. Set CodeQL `upload: never` so database generation and analysis still execute in CI without relying on unavailable code-scanning entitlement.
- The next CI run then exposed a separate Rust CodeQL analyzer prerequisite: it is marked experimental by the pinned CodeQL action and requires `CODEQL_ENABLE_EXPERIMENTAL_FEATURES=true`. Enable that environment flag only for the Rust matrix entry; keep CodeQL SARIF upload disabled.
- The spell-check failure is a Vietnamese fixture word (`Nam`); add it to `_typos.toml`'s allowed words.
- Locally, `actionlint` passes for all three workflows and `pnpm release:verify` passes 11/11. The follow-up Rust CodeQL configuration is awaiting its GitHub run. Source/release repository alignment, clean reviewed commit, Windows signing, runtime trust/catalog configuration, updater client, and native Windows/macOS acceptance remain open.

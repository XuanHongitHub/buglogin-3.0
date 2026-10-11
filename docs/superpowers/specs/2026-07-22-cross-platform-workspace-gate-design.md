# Cross-platform workspace release gate

- Date: 2026-07-22
- Owner: IO-RELEASE
- Status: implementation

## Goal

CI must exercise the rebuilt BugLogin workspace on the pinned Windows and macOS runners used by the release contract. Existing Donut-era workflows are retained as reference/compatibility checks, but cannot be the only evidence for `crates/**` and `apps/desktop/**`.

## Required checks

- `cargo test --workspace --all-targets`
- `cargo check -p buglogin-desktop-app --all-targets`
- desktop model/bridge self-tests and TypeScript typecheck
- desktop frontend production build
- targeted Windows and macOS platform tests where available
- migration and API package suites included by workspace tests

## Non-goals

- No Docker or live PostgreSQL/Redis/S3 claim in this gate.
- No replacement of native signed-artifact acceptance evidence.
- No production cutover authorization.

## Evidence

Each matrix job reports the exact runner/target in GitHub Actions logs. Native signed install, proxy, cookie, sync, and migration journeys remain separate release gates.


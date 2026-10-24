# Stable-only desktop release implementation plan

## Status

In progress. No 3.0.6 tag, stable object, or public release is created by this plan until acceptance evidence exists.

## Scope

Implement the stable-only release design in `docs/superpowers/specs/2026-10-24-stable-only-desktop-release-design.md` across the Malvanut product source and XuanHong Actions repository. Preserve both repo identities and the protected GitHub signing/S3 environments. Do not change API cleanup/deploy helpers, runtime/MCP behavior, or production DNS.

## Phases

1. Align updater/download manifest assembly and landing route on the separate `latest.json` / `downloads.json` contract; add focused schema and feed checks.
2. Change the candidate workflow to produce only a private Actions artifact after signing; remove public RC/S3/release side effects.
3. Replace RC promotion with protected stable publication bound to the successful candidate run, exact tag/source SHA, immutable artifact hashes, and reviewed Windows/macOS acceptance evidence.
4. Update workflow policy tests, release documentation, and current repo synchronization checklist.
5. Run focused release verifier/tests and workflow lint; commit/push the product source to Malvanut main and sync the release implementation to the XuanHong Actions repo without force-push.
6. Dispatch candidate build only after the Actions repo contains the exact 3.0.6 source. Do not publish stable until physical/dedicated Windows and macOS acceptance evidence is reviewed.

## Verification record

- Static/workflow checks: pending.
- Windows candidate install/update/profile/sync acceptance: pending.
- macOS candidate install/update/profile/sync acceptance: pending.
- S3 stable public readback: pending.

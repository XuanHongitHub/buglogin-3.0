# Manifest Commit Idempotency Plan

Spec: `docs/superpowers/specs/2026-10-19-manifest-commit-idempotency-design.md`.

- [x] Derive the desktop manifest commit key from the full request identity and add regression coverage for lease/revision changes.
- [x] Document the desktop-login `500 Problem` response in canonical OpenAPI and keep generated contracts aligned.
- [x] Run focused Rust tests, desktop tests/typecheck, API generation check, contract tests, and `git diff --check`.

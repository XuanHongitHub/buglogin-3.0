# Owner Bootstrap Environment Guard Plan

Spec: `docs/superpowers/specs/2026-10-17-owner-bootstrap-environment-guard-design.md`.

- [x] Inspect the existing first-owner API, transactional persistence, and bootstrap CLI.
- [x] Add a fail-closed production environment check before password prompting or database access.
- [x] Add focused source regression coverage for accepted and rejected environment configurations.
- [x] Include the bootstrap binary in the supported API release image and document its operator-only execution boundary.
- [ ] Run focused tests during the consolidated verification pass requested by the owner.

The hosted database and existing imported records are not modified by this work.

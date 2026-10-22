# Operator Platform Admin Password Reset Plan

Spec: `docs/superpowers/specs/2026-10-17-operator-admin-password-reset-design.md`.

- [x] Inspect hosted recovery/password/session/token persistence and the first-owner CLI boundary.
- [x] Add one atomic API-state operation restricted to an active durable platform admin.
- [x] Add a separate hidden-prompt operator CLI with exact production and target confirmation guards.
- [x] Add a read-only CLI mode to identify durable platform administrators before password reset; source-only and not built or run against production.
- [x] Add source regression coverage for operator listing identity/status output and absence of credential fields; execution is deferred to the consolidated verification pass.
- [x] Package the operator CLI with the API release image and document host-only execution.
- [x] Add source regression coverage for password replacement; run the regression suite during the consolidated verification pass.
- [ ] Do not execute the reset against any production account as part of this source task.

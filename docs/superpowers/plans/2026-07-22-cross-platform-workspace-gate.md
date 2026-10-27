# Cross-platform workspace gate plan

- Date: 2026-07-22
- Status: implemented; native CI evidence pending

## Steps

- Add reusable platform workspace workflow
- Wire gate into main CI status
- Validate YAML and local policy tests
- Run local workspace and desktop checks
- Record native evidence limitations

## Acceptance

- Windows 2025 and macOS 14/13 matrix executes rebuilt workspace paths.
- Main CI fails when platform gate fails.
- Local static checks pass.
- No production-ready claim without native runner evidence.


## Follow-up hardening

- Package upload writes journal intent before CAS object and repairs missing objects on exact retry.
- Historical unjournaled objects still require provider lifecycle/reference scan policy before production.


- Hosted profile reads now project durable runtime state (unning/stopped) without mutating profile revision.


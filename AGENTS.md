# BugLogin 2.0 — Project Rules

## Status

Discovery/Gate 0. Production implementation is blocked until the readiness checklist passes.

## Source of truth

- Product/spec: `docs/superpowers/specs/`
- Execution plan: `docs/superpowers/plans/`
- Architecture/ADR: `docs/architecture/`
- Contracts: `docs/contracts/`
- Design system/IA: `DESIGN.md`, `docs/design/`
- UI authority: `docs/design/ui-authority.md` (binding; read before UI work)
- Capabilities: `docs/capabilities/`
- Agent ownership: `docs/agents/`
- Migration: `docs/migration/`
- E2E/release: `docs/e2e/`, `docs/release/`
- References only: `.tmp/`

## Non-negotiable rules

- `.tmp/` is read-only reference space; never edit or delete source references.
- Do not touch `E:\bug-login` directly.
- Rust owns filesystem, processes, networking, storage, permissions, state transitions, REST, and MCP authority.
- TypeScript/React owns presentation only; frontend checks are not security.
- Every capability needs schema, permission, API, MCP, audit, UI states, tests, and migration impact.
- Every agent follows `docs/agents/work-packet-ownership.md`; no overlapping write scopes.
- No production stubs, fake data, dead buttons, silent failures, or unapproved scope.
- No production migration without snapshot, dry-run, rehearsal, verification, and rollback proof.
- No release claim without Windows and macOS evidence.
- Donut upstream is the desktop visual and interaction baseline. Do not invent a second desktop CSS system or treat static mockups as production references.
- Web may use mode-specific layouts, but desktop/web must share the approved typography, semantic tokens, control geometry, status taxonomy, icon rules, and accessibility behavior.
- UI work is blocked until the screen IA row, capability/state contract, approved reference package, and screenshot/interaction gate are present.
- Static prototypes with fake records, simulated jobs, fake connection state, or fake progress are references only and cannot be copied into production paths.

## Agent workflow

1. Read the relevant artifact and ownership row.
2. Implement only the assigned write scope.
3. Add targeted tests/evidence.
4. Report files, contracts, tests, residual risk.
5. Stop on contract conflict; do not invent a second authority.
6. For UI packets, read `docs/design/ui-authority.md`, inspect the pinned Donut reference, and report deviations before editing.

## Verification

Smallest meaningful gate first. Shared behavior requires broader E2E. UI changes require screenshot and interaction QA. Security/data-loss/permission failures block the wave.

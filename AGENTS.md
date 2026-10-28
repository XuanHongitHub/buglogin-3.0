# BugLogin 2.0 — Project Rules

## Status

The readiness gates govern release and production promotion. They do not block scoped implementation, local testing, or completing a vertical feature through the existing architecture.

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
- A missing endpoint, command handler, persistence path, or UI binding is unfinished implementation, not a blocker. Trace the existing contract and neighboring code, then implement the missing vertical slice in its owning layers and test it.
- Treat 401/403/404/5xx as evidence to diagnose. Inspect the request, effective identity/scopes, route, policy, and server response; fix defects within the authorized source and configuration. Preserve intentional denials and report the precise reason. Do not stop at “not found,” “forbidden,” or “unsupported.”
- Stop only for a verified external dependency or authority conflict that cannot be resolved from the available source/configuration, or when the next action would be destructive/irreversible without required evidence. First exhaust local code, tests, docs, and available runtime diagnostics; then state the exact attempted path and remaining external dependency.
- No production migration without snapshot, dry-run, rehearsal, verification, and rollback proof.
- No release claim without Windows and macOS evidence.
- Build disk pressure must be handled automatically before it blocks work: check free space and active build processes, then reclaim only disposable build caches with the owning tool (for example, `cargo clean --target-dir <verified-target-dir>`) or move the build to a volume with enough space. Never remove source, user data, `.tmp/` references, installed runtimes, or release artifacts as cleanup. Verify the reclaimed space and resume the interrupted check.
- Donut upstream is the desktop visual and interaction baseline. Do not invent a second desktop CSS system or treat static mockups as production references.
- Web may use mode-specific layouts, but desktop/web must share the approved typography, semantic tokens, control geometry, status taxonomy, icon rules, and accessibility behavior.
- For a new UI surface, inspect the IA/state/design authority and create any missing approved artifacts as part of the requested work when scope allows. Missing artifacts do not block implementation by themselves. Screenshot/interaction/accessibility gates remain required before accepting or releasing the UI change.
- Static prototypes with fake records, simulated jobs, fake connection state, or fake progress are references only and cannot be copied into production paths.

## Agent workflow

1. Read the relevant artifact and ownership row.
2. Implement only the assigned write scope.
3. Add targeted tests/evidence.
4. Report files, contracts, tests, residual risk.
5. Stop on contract conflict; do not invent a second authority.
6. For UI packets, read `docs/design/ui-authority.md`, inspect the pinned Donut reference, and report deviations before editing.
7. Complete the authorized vertical slice before reporting: connect UI → desktop command/client → API → authorization → durable storage, add focused regression tests, and diagnose/fix failures encountered along that path.

## Verification

Smallest meaningful gate first. Shared behavior requires broader E2E. UI changes require screenshot and interaction QA. Security/data-loss/permission failures block the wave.

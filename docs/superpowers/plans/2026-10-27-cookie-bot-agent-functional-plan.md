# Cookie Bot and Agent Functional Plan

Date: 2026-10-27
Status: implementation in progress; placeholder success/empty responses have been removed locally, but the real worker/runtime and end-to-end execution are not implemented.
Scope: make the existing Cookie Bot and Agent pages perform the actions they present, through the established desktop/Rust/API contracts. This is not a page redesign.

## Findings from the current source

The pages are present and wired into the desktop shell, but several user-visible operations terminate in placeholder Tauri handlers. This explains why the sections can render while actions appear to do nothing or return empty state.

### Cookie Bot

- UI entry: `src/components/cookie-bot-page.tsx`; shared state and live remote-session subscription: `src/hooks/use-cookie-bot.ts`.
- The desktop command names and DTO wrappers are in `src/lib/cookie-bot.ts`; handlers are registered in `apps/desktop/src-tauri/src/lib.rs` and implemented in `apps/desktop/src-tauri/src/agent_features.rs`.
- The SQLite-backed schedule/template CRUD exists in `agent_features.rs`, but `check_cookie_bot_conflicts` always returns no conflicts, `get_cookie_bot_runs` always returns an empty list, `run_cookie_bot_now` reports a fabricated started run, `cancel_cookie_bot_run` succeeds without cancelling, and usage/quota return fixed values.
- Existing remote-session state is consumed by the hook. The connected-cookie-job design explicitly says that API queue tracking does not itself implement desktop inbox polling/retry or local cookie mutation. It also records the Wayfern encryption, journaling, idempotency, and recovery requirements. See `docs/superpowers/specs/2026-10-13-connected-cookie-job-boundary-design.md`.

### Agent

- UI entry: `src/components/agent-page.tsx`; state/actions: `src/hooks/use-agent-console.ts`; command wrappers: `src/lib/agent-console.ts`, `src/lib/recipes.ts`, and `src/lib/schedules.ts`.
- `apps/desktop/src-tauri/src/agent_features.rs` currently returns an empty Agent console snapshot, has no-op request/note/takeover/handback/activity/window commands, and returns empty schedule-run history. Pause state is only in process memory.
- Recipe and schedule definitions have local SQLite CRUD, but “run now” reports a fabricated run ID/status and the notice test is a no-op. Persisting a recipe is therefore not evidence that an agent can execute it.
- `apps/desktop/src-tauri/src/lib.rs` registers these commands, so the shell can call them successfully while receiving fake/empty behavior.

## Outcome and boundaries

Deliver an honest, usable vertical slice for each existing page:

1. A button is enabled only when the underlying capability is available; otherwise the UI shows the backend-derived reason and next action.
2. Every accepted operation has durable or explicitly live state, a real result, cancellation/recovery semantics where applicable, and audit/permission enforcement.
3. Empty state means the backend confirmed there is no data. It must not be used as a fallback for a missing service, disconnected worker, or failed request.
4. Cookie values, encryption keys, filesystem paths, and vault references remain device-local. Never log or send cookie contents to the API.
5. Do not imply autonomous browsing/automation is running until the supported worker/runtime is connected and the operation is observable end to end.

## Implementation sequence and code locations

### Wave 0 — contracts and ownership

- Read/update the relevant rows in `docs/design/information-architecture.md`, the UI state matrix under `docs/design/`, `docs/capabilities/registry.md`, `docs/contracts/`, and `docs/agents/work-packet-ownership.md` before implementation.
- Add or update capability specs for Cookie Bot enrolment, scheduling, execution, run history/cancellation, and Agent session/recipe/schedule execution. Define which authority owns each state (API, device worker, or local-only) and permission/audit requirements.
- Keep non-overlapping packet ownership: API/control contracts (`IO-AUTO`/`IO-CONTROL`), device execution and persistence (`IO-CORE`/`IO-DATA`), UI (`IO-UI`), and vertical integration/E2E (`IO-QA`). Integrate through named contract owners; do not have parallel writers edit `agent_features.rs` or shared DTOs.

### Wave 1 — Cookie Bot truthful state and real run lifecycle

- Trace and complete the path from `src/components/cookie-bot-page.tsx` and `src/hooks/use-cookie-bot.ts` through `src/lib/cookie-bot.ts`, Tauri registrations in `apps/desktop/src-tauri/src/lib.rs`, and handlers in `apps/desktop/src-tauri/src/agent_features.rs`.
- Replace the fake conflict/run/cancel/usage responses with real service-backed behavior. If a service is unavailable, return a typed unavailable/error state; never fabricate a run ID or report success.
- Reuse the established remote-session client/state path (`src/lib/remote-sessions.ts` and its existing Rust/API owner) for remote execution and live state. Avoid a second session/status store.
- Complete the desktop side of the queue contract in `docs/superpowers/specs/2026-10-13-connected-cookie-job-boundary-design.md`: authenticated inbox polling, claim/ack/complete, retry/backoff, idempotent recovery, stop/lease proof, and the local Wayfern cookie mutation/journal transaction. Do not claim completion from enqueue acceptance alone.
- Keep Cookie DB writes inside the existing cookie authority/profile manager boundary (`apps/desktop/src-tauri/src/profile_cookie_manager.rs` and related existing modules); verify schema/key/profile lifecycle before mutation. Do not put secrets in logs or API receipts.
- Derive enrolment, permissions, team scope, quota, schedule conflicts, and usage from authoritative services. Remove fixed values and local-only assumptions from `agent_features.rs` once the canonical owner is connected.
- UI files: `src/components/cookie-bot-page.tsx`, `cookie-bot-overview.tsx`, `cookie-bot-schedule.tsx`, `cookie-bot-activity.tsx`, `cookie-bot-enrol-dialog.tsx`, `cookie-bot-shared.tsx`; preserve current layout and add explicit loading, empty, offline/degraded, forbidden, conflict, queued, running, retrying, cancelled, failed, and completed states.

### Wave 2 — Agent real session and operation lifecycle

- Trace the existing Agent event/session protocol from `src/hooks/use-agent-console.ts` and `src/lib/agent-console.ts` to the Rust event producers and remote bridge/session implementation. The page should subscribe to real snapshots/events; `get_agent_console` must not return a hard-coded empty console.
- Replace no-op handlers in `apps/desktop/src-tauri/src/agent_features.rs` with authorized operations against the canonical Agent service. Request answer/dismiss, notes, profile takeover/handback, pause, clear activity, and show-window must return explicit outcomes and persist/audit state where the contract requires it.
- Persist pause and recovery state according to the service authority, so app restart does not silently resume or discard a safety pause. Define conflict behavior for simultaneous operators.
- Recipes: keep existing editor/CRUD in `src/components/agent-recipes.tsx`, `agent-recipe-steps.tsx`, `agent-recipe-recorder.tsx` and `src/lib/recipes.ts`, but connect record/start/stop and execute to a real supported browser/runtime operation. A saved recipe must have schema/version validation and safe failure reporting.
- Schedules: connect `src/lib/schedules.ts` and schedule UI to an actual scheduler/worker; persist each run, progress, terminal result, cancellation, and retry decision. Replace fabricated `triggered` receipts and empty run history in `agent_features.rs`.
- Keep Agent activity, profile holds, remote sessions, and quota backed by the same authoritative event/service state. Do not synthesize sessions, quota, status, or activity in the renderer.
- UI files: `src/components/agent-page.tsx`, `agent-conversation.tsx`, `agent-profiles.tsx`, `agent-activity.tsx`, `agent-recipes.tsx`, `agent-recipe-recorder.tsx`, plus `src/hooks/use-agent-console.ts`; display disconnected, connecting, permission denied, stale, paused, running, and failed states with safe actions.

### Wave 3 — API/MCP/audit and contract alignment

- Locate the existing route/service/schema owners before adding endpoints. For cookie jobs, extend the established services around `crates/api/src/services/connected_cookie_jobs.rs`; do not invent a parallel job queue.
- Ensure server authorization checks the authenticated actor, organization/workspace/profile binding, capability, and device identity on every operation. Add audit events for operator actions and execution transitions. MCP must use the same command registry/policy if an equivalent MCP operation is exposed.
- Update canonical contracts/OpenAPI and generated clients through the repository's generation path; never hand-edit generated DTOs.
- Verify idempotency, replay, worker disconnect, app restart, revoked access, profile archived/locked/running, and partial completion behavior.

### Wave 4 — verification and release gate

- Rust targeted tests for command/service behavior, durable state, authorization and recovery; contract/OpenAPI coverage and generated-client cleanliness.
- Desktop tests/typecheck for command DTOs and UI states; E2E for both pages using real local API/worker fixtures, including offline/degraded and permission-denied cases.
- UI changes require keyboard/focus and responsive interaction checks plus screenshots against `docs/design/ui-authority.md` and the pinned Donut reference.
- Cookie execution requires Windows and macOS evidence for actual browser cookie mutation and crash recovery. Agent execution requires evidence from the supported Agent bridge/runtime, not only a successful Tauri invoke.
- Do not release a page as “working” if its worker/runtime or API dependency is absent; show the precise unavailable reason in-product instead.

## Acceptance checklist

- [ ] No fake success, fabricated IDs/counts/quota, empty-on-error fallback, or no-op action remains on either page.
- [ ] UI state maps to authoritative state and stays correct across restart, reconnect, permission change, and concurrent operator actions.
- [ ] Cookie jobs are idempotent and recoverable; plaintext cookie data and keys remain device-local.
- [ ] Agent requests, holds, recipes, and schedules have real authorization, durable/observable outcomes, and audit coverage.
- [ ] Loading, empty, error, unavailable, forbidden, stale, conflict, running, and terminal states are tested.
- [ ] Targeted backend, desktop, contract, and end-to-end gates pass; Windows/macOS execution evidence is recorded before release.

## Implementation progress — 2026-10-27

- Replaced Agent console/action, recording, remote-session, and schedule-run placeholder responses with typed `AGENT_SERVICE_UNAVAILABLE` errors until a supported Agent service is connected. Local recipe/schedule definitions remain available as persisted configuration, not execution receipts.
- Replaced Cookie Bot run/history/cancel/conflict/quota/usage placeholder behavior with typed `COOKIE_BOT_WORKER_UNAVAILABLE` errors. Local schedule/template CRUD remains persisted configuration only.
- Reconciled the connected-cookie job implementation before classifying the remaining gap: a desktop inbox worker already polls authenticated jobs, validates device/profile binding, claims and acknowledges, acquires the profile lease, journals an idempotent local mutation, materializes the stopped Wayfern cookie database, syncs/stops, and completes or fails the API job. This is the import/copy execution path and is separate from the Cookie Bot schedule/run UI, which still lacks an actual automation worker. Cross-device source copy remains explicitly rejected until its source transport exists.
- `cargo test --locked -p buglogin-api connected_cookie_jobs --lib` passes all 12 queue/state-machine tests, including durable replay, device scoping, cancellation/expiry, receipt binding, redacted failure and recovery visibility.
- Added localized Agent and Cookie Bot worker-unavailable messages. Agent console read failures now remain visible in the page with retry instead of rendering as an empty successful snapshot. Cookie Bot run refresh failures retain last-known rows and surface a retryable error.
- Desktop typecheck, targeted Biome lint (formatter disabled because repository files use CRLF), Rust formatting check for `agent_features.rs`, and `cargo check -p buglogin-api --lib` passed. Full desktop Rust check is still blocked by the missing bundled `binaries/buglogin-migrate-x86_64-pc-windows-msvc.exe` resource in this checkout.
- Remaining: connect the supported Agent and Cookie Bot workers through the existing API/event contracts; implement durable run lifecycle, auth/audit, recovery, actual browser cookie mutation, cancellation, and Windows/macOS E2E evidence. These features are not production-ready merely because the UI now reports the missing dependency accurately.

# Resource Management Rebuild Plan

Status: in progress

## Phase 0 — Source and ownership

- [x] Read UI authority, IA, state matrix, API/domain model, ownership manifest, current DES-02/DES-03, and current route implementations.
- [x] Confirm no active AGY writer: `codex_chat.md` records the prior implementation as complete; current edits are in this shared release worktree.
- [x] Confirm pinned Donut reference exists at `F:\Herd\buglogin\.tmp\donutbrowser-upstream` (read-only; local clean worktree does not contain the `.tmp` reference).
- [x] Trace current typed desktop API surface before making route/action claims.
- [x] Use the pinned Donut semantic tokens and shared Dialog/Drawer, Button, Table, Tabs, and DropdownMenu primitives; keep the existing desktop shell. A dedicated screenshot comparison is still outstanding.

## Phase 1 — One navigation and context model

- [x] Keep one Organization/Workspace context switcher and remove duplicate context/quota/role ribbons.
- [x] Make Organization and Workspace views canonical, linkable routes with breadcrumbs and back/forward handling.
- [x] Move Group/Profile/Member/Account actions under their real parent scope and remove the old standalone group-management route.
- [x] Keep Platform Admin separate and label its scope as API-authorized visibility; do not infer global access from tenant-visible rows.

## Phase 2 — Rebuild operational surfaces

- [x] Organization directory and detail: create/update/list workspaces; no invented org membership actions.
- [x] Workspace directory and detail: create/update/switch, compact resource navigation, members/groups/profiles.
- [x] Group/Profile: supported CRUD, shares, assignments, and lifecycle operations with refresh/error states.
- [x] Member/Account: distinct invite and direct provisioning; typed role choices; workspace-scoped status and role changes.
- [x] Platform Admin: API-backed account/resource operations currently exposed by the contract; label scoped visibility honestly. Resource update/archive is absent from the current API, so no fake controls were added.
- [x] Replace old group route and workspace modal entry point with canonical page flows and shared drawer forms.

## Phase 3 — Verify each vertical workflow

- [ ] Trace every visible action through typed client → Rust command/API → permission → durable store.
- [ ] Verify owner/admin/lead/operator/member/platform role behavior and denied responses.
- [ ] Verify create, update, delete/archive, account provision, invite, role/status change, group share, profile open/stop/sync as applicable.
- [ ] Verify stale data refresh, loading/empty/error/forbidden/offline/conflict states, keyboard, responsive layout, and no page-level scroll.
- [ ] Run targeted typecheck and self-tests; capture desktop screenshots and interaction evidence at supported viewport sizes.
- [ ] Update this checklist and report unsupported backend operations precisely. Do not release until all release gates are met.

## Current execution focus

Implementation is in the release worktree. Typecheck, desktop self-tests, Biome, and production frontend build pass. The local API preflight and Tauri native dev build/launch pass on isolated port 1421; the pre-existing process on 1420 was left untouched. Authenticated role-by-role action checks and screenshot evidence remain open. No release has been made.

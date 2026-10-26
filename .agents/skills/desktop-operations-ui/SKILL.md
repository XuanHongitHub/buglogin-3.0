---
name: desktop-operations-ui
description: Use when building, modifying, or reviewing desktop application layouts, command bars, split panes, tables, drawers, or Tauri-backed frontend surfaces. Enforces Donut-compatible operational density, local-first state presentation, and desktop ergonomics.
---

# Desktop Operations UI & Ergonomics

Rules and design patterns for high-density desktop operational surfaces. Aligned with Donut upstream authority, Tauri 2 integration, and modern desktop workstation UX (Linear, Raycast, Donut).

---

## 1. Desktop Window & Layout Architecture

```
+--------------------------------------------------------------------------+
| Window Titlebar (Drag Region, Traffic Lights / Window Controls, Profile) |
+--------------------------------------------------------------------------+
| App Rail | Workspace Sidebar | Command Bar / Toolbar Filters             |
|          |                   +-------------------------------------------+
| (Icons)  | (Context Tree,    | Primary Data Table / Grid                 |
|          |  Resource Groups, | (Compact Density, Sticky Header,          |
|          |  Settings)        |  Multi-select, Keyboard Navigable)        |
|          |                   +-------------------------------------------+
|          |                   | Inspector / Detail Drawer (Split-pane)    |
+----------+-------------------+-------------------------------------------+
| Status Bar (Local-first sync engine, proxy health, active workers)       |
+--------------------------------------------------------------------------+
```

### Layout Constraints
- **Fixed Window Container:** The root viewport is fixed (`overflow-hidden`). Never allow the entire desktop window to scroll like a standard webpage.
- **Internal Scroll Regions:** Wrap long lists and tables inside `<ScrollArea>` primitives with custom slim scrollbars.
- **Split-Panes & Drawers:** Prefer slide-out side drawers (`Sheet` / `Drawer`) or collapsible inspector split-panes over full-screen page navigation to maintain operational context.

---

## 2. Density & Hierarchy

- **Compact Technical Density:** Desktop operations require information density. Avoid excessive padding or giant consumer-SaaS cards.
- **Table Standards:**
  - Sticky header row with clear column sorting indicators.
  - Row height: Compact (`h-9` or `h-10`).
  - Right-aligned numbers, monospaced fonts for IDs and technical hashes (`font-mono text-xs`).
  - Truncated text with full value visible on hover tooltip.
  - Action column sticky to the right edge.

---

## 3. Keyboard-First Navigation

- **Command Palette (`Ctrl+K` / `Cmd+K`):** Quick switcher for profiles, workspaces, settings, and immediate operational actions.
- **List Navigation:** Support arrow key navigation (`Up`/`Down`) across table rows and search results.
- **Global Shortcuts:**
  - `Esc`: Close open modal/drawer/popover.
  - `Enter`: Trigger primary action / open selected item detail.
  - `Ctrl/Cmd + N`: Create new resource.

---

## 4. Local-First Truth & State Legibility

Desktop applications run locally with backend processes (Tauri / Rust). State transitions must reflect true local-first operations:

```
[Local State] ---> [Queued] ---> [Syncing] ---> [Synced]
                             ---> [Conflict]
                             ---> [Policy-Blocked]
```

- **Clear Badges:** Differentiate between `local`, `queued`, `syncing`, `synced`, `error`, and `policy-blocked` using subtle badge styles.
- **Optimistic UI with Rollback:** Reflect user changes immediately in the UI; if the underlying Rust command or network sync fails, roll back smoothly and trigger an actionable error toast.
- **Zero Silent Failures:** Every Tauri `invoke` error must be captured in `try...catch` and surfaced with a human-readable diagnosis.

---

## 5. Verification Checklist

Before accepting a desktop UI view:
- [ ] Viewport does not trigger window-level scrollbars.
- [ ] Uses Radix/Shadcn primitives mapped to OKLCH tokens.
- [ ] Handles rapid window resizing gracefully.
- [ ] Keyboard navigation and focus rings are fully functional.
- [ ] All error and empty states provide a recovery action.

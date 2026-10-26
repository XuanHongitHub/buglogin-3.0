---
name: ui-ux-designer
description: Use when designing, building, reviewing, or refactoring frontend interfaces, components, forms, tables, modals, or page layouts. Enforces strict design tokens, zero arbitrary styling, WCAG 2.1 AA accessibility, and comprehensive state-driven UI patterns derived from industry-standard AI designer rules.
---

# UI/UX Designer & Design System Intelligence

A specialized design skill for production-grade web and desktop interfaces. Incorporates modern standards from `cursor-designer`, `shadcn/ui`, and high-density operational applications (Linear/Raycast).

---

## 1. Core Design Philosophy

- **Zero "AI-Slop":** Avoid generic, oversized, glossy, or decorative templates. Every element must serve an operational purpose.
- **Progressive Hierarchy:** Visual hierarchy guides user attention: Primary Action -> Contextual Tools -> Data Display -> Secondary Metadata.
- **Composition over Monoliths:** Assemble views from small, composable primitives rather than huge all-in-one components.

---

## 2. Token & Color Discipline (Strict)

- **NEVER use arbitrary or hardcoded values:**
  - ❌ Prohibited: `text-red-500`, `bg-[#1a1a1a]`, `mt-[13px]`, `w-[320px]`
  - ✅ Required: `text-destructive`, `bg-background`, `mt-3`, `w-80`
- **Semantic Color Matrix (OKLCH):**
  - **Surfaces:** `bg-background`, `bg-card`, `bg-popover`, `bg-muted`
  - **Typography:** `text-foreground`, `text-muted-foreground`, `text-primary`, `text-destructive`
  - **Borders & Focus:** `border-border`, `ring-ring`, `outline-none focus-visible:ring-2`
  - **Actions:** `bg-primary text-primary-foreground`, `bg-secondary text-secondary-foreground`, `bg-destructive text-destructive-foreground`
- **Theme Awareness:** Respect automatic system dark/light switching via CSS variables. Never hardcode conditional `dark:bg-black` when semantic classes already resolve colors.

---

## 3. The 8 Mandatory UI States Matrix

Every data-bound view, table, drawer, or modal must explicitly account for all 8 lifecycle states:

```
[1. Uninitialized] ---> [2. Loading/Skeleton] ---> [3. Empty (Zero data)]
                                              ---> [4. Populated (Happy path)]
                                              ---> [5. Syncing / Background Job]
                                              ---> [6. Degraded / Network Fail]
                                              ---> [7. Conflict / Out-of-sync]
                                              ---> [8. Forbidden / Denied ACL]
```

1. **Uninitialized / Initializing:** Subtle pulse or neutral placeholder.
2. **Loading:** Accurate skeleton loaders matching the exact shape of incoming content (prevent layout shifts / CLS).
3. **Empty:** Helpful empty states with an explanatory label, an illustration/icon, and a clear call-to-action (CTA).
4. **Populated:** Dense, legible data display with proper pagination, sorting, and row actions.
5. **Syncing / In-Flight:** Subtle indicator (spinner badge, progress toast) without locking the entire UI unless required.
6. **Degraded / Error:** Explicit inline banner or toast explaining the failure reason with a "Retry" trigger.
7. **Conflict / Out-of-Sync:** Clear visual diff or resolution action drawer.
8. **Forbidden / ACL Denied:** Clear explanation of missing permissions; do not leave a blank white container.

---

## 4. Accessibility (a11y) — WCAG 2.1 AA Compliance

- **Semantic HTML:** Use `<button>`, `<nav>`, `<main>`, `<header>`, `<article>` instead of clickable `<div>` elements.
- **Keyboard Navigation:**
  - All interactive elements must receive focus with a visible focus ring (`focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1`).
  - Support `Escape` to dismiss modals, popovers, and drawers.
  - Support `Enter` / `Space` on custom controls.
- **Screen Reader Readiness:**
  - Provide `aria-label` on icon-only buttons (e.g., `<button aria-label="Close dialog"><X className="size-4" /></button>`).
  - Maintain correct `aria-expanded` and `aria-controls` on collapsibles and accordions.

---

## 5. Micro-Interactions & Form Feedback

- **Button Actions:** Any asynchronous action must display an inline loading spinner inside the button and set `disabled={isLoading}` to prevent duplicate submissions.
- **Toasts:** Use centralized toast wrappers (e.g., Sonner). Never rely solely on `console.error` or native `window.alert()`.
- **Transitions:** Keep micro-transitions subtle and snappy (`transition-colors duration-150`, `ease-out`). Avoid long, sluggish animations (> 300ms).

---

## 6. Internationalization (i18n)

- Never hardcode raw English or Vietnamese strings directly into UI templates.
- Always route user-facing text through localization hooks (`useTranslation()` / `t("key")`).

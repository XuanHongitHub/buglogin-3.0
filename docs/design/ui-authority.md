# BugLogin UI Authority

Status: binding implementation contract
Owner: Product Design + Desktop Platform
Last updated: 2026-08-11

## Purpose

This document prevents design drift between the Donut desktop core, the BugLogin desktop application, and the hosted web surfaces. It is the authority agents must read before changing UI, CSS, layout, or screen composition.

## Authority order

Management flow decision (2026-10-26): the canonical screen/action specification is [Organization, account and resource management UI/UX](../superpowers/specs/2026-10-26-management-ui-ux-design.md). It supersedes conflicting management layouts in DES-02/DES-03 and older ASCII plans: sidebar context switcher, content-toolbar actions, no archive/offline controls, no profile lifecycle in management inventories, and complete single/bulk account and access workflows. Backend contracts remain authoritative for identity, role, scope, and persistence; the new specification records required implementation rather than claiming capabilities have shipped.

1. Product behavior, permissions, state, and backend contracts.
2. The pinned Donut upstream source at `.tmp/donutbrowser-upstream` and `docs/architecture/upstream-baseline-manifest.yaml` for desktop shell, component behavior, density, theme mechanics, and interaction patterns.
3. This document for BugLogin-specific extensions and web/desktop boundaries.
4. `DESIGN.md`, `docs/design/information-architecture.md`, and `docs/design/screen-state-matrix.md` for screen-level composition and state coverage.
5. Approved implementation screenshots and interaction evidence.

No static mockup, generated screenshot, or agent preference can override this order.

## Desktop rule

The native desktop app is a Donut-compatible product surface, not a new visual language. Preserve the upstream patterns for:

- app rail and navigation density;
- profile/group filtering and compact toolbar behavior;
- tables, row actions, selection, and status presentation;
- drawers, dialogs, command palette, focus handling, and keyboard shortcuts;
- semantic theme variables, typography, spacing, radii, and icon family.

BugLogin may add workspace, sync, ACL, migration, job, and diagnostic controls, but additions must use the upstream component and token language. Do not introduce a second raw-CSS shell, an unrelated palette, or a second spacing scale.

The desktop implementation must use the existing upstream theme mechanism or a typed BugLogin token layer mapped to it. Hardcoded page-wide colors, one-off CSS design systems, and raw static HTML screens are prohibited in production paths.

## Web rule

Web surfaces may change density and shell structure by mode:

- public web: more whitespace and product explanation;
- account: focused settings and security workflows;
- workspace: compact operational navigation and tables;
- platform admin: austere, audit-heavy operations.

These modes are not separate brands. They must share the Donut-compatible typography, semantic color roles, control geometry, status language, icon rules, focus behavior, and state taxonomy. Web may use its own responsive layout, but it may not invent an unrelated visual system.

## Forbidden implementation shortcuts

- Do not treat a static prototype or screenshot with fake records as production UI evidence.
- Do not copy prototype-only CSS into production without tracing each token and component to the upstream or shared design system.
- Do not use fake counts, fake connection states, fake PIDs, fake jobs, or simulated progress in production screens.
- Do not hide missing capability behind a disabled or absent control; show the backend-derived reason and safe next action.
- Do not add a new shell, color palette, typography family, icon family, radius scale, or card grammar without an ADR and design approval.
- Do not implement a screen until its IA row, state matrix, capability contract, and approved reference package exist.

## Identity Labels

- Standard user-facing UI must use a verified person name, email address, device name, or a localized generic label. Never use account, workspace, member, device, profile, or resource IDs as ordinary labels, tooltips, or empty-value fallbacks.
- Never invent or infer a person's identity or device name. If the source does not provide a verified display value, omit the identity or show a localized unavailable state.
- IDs may appear only in an explicitly technical diagnostic surface where they are necessary to resolve a support issue; they must not leak into routine product workflows.

## Required evidence before resume

Every UI work packet must include:

1. upstream/reference components inspected;
2. screen route, role, capability, and primary job;
3. real or sanitized fixture with long-label coverage;
4. loading, empty, error, forbidden, offline/degraded, locked/conflict, and success behavior where applicable;
5. keyboard/focus and responsive behavior;
6. screenshot and interaction evidence at the assigned viewports;
7. explicit deviations from upstream, with owner and rationale.

If the reference package or authority is unclear, stop and report the conflict. Do not invent a replacement.


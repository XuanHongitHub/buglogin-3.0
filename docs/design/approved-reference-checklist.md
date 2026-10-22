# BugLogin 2.0 Approved-Reference and Screenshot Checklist

Status: approval gate for production UI  
Last updated: 2026-07-20  
Parent: [DESIGN.md](../../DESIGN.md)

## Binding authority

Read [UI authority](ui-authority.md) before preparing or reviewing any reference. The pinned Donut source is the desktop baseline. A static prototype, screenshot, or generated mockup is not an approved reference by itself and must never be treated as production UI evidence.

Before a screen enters implementation, the package must identify the upstream/shared components it reuses and list every intentional deviation. If the package conflicts with `docs/design/ui-authority.md`, implementation stops until the conflict is resolved.

## 1. Approved-reference package

No screen enters implementation from prose alone. Each major screen needs an approved reference package containing:

- Screen ID and route.
- User role and exact capability set.
- User job and success condition.
- Realistic redacted data fixture, including long names and large counts.
- Desktop and responsive composition.
- Loading, empty, error, forbidden, offline/degraded, locked/conflict, and success references where required by the state matrix.
- Interaction notes for primary, bulk, destructive, and keyboard actions.
- Backend authority fields displayed or used.
- Copy approved for destructive/security-sensitive outcomes.
- Accessibility annotations: heading order, landmarks, focus order, live regions, labels.
- Screenshot viewport assignments.
- Approval owner, date, version, and unresolved deviations.

Reference artifacts may be high-fidelity design files, implementation-native HTML prototypes, or annotated screenshots. Static image-only references are insufficient for complex keyboard, table, dialog, migration, or responsive behavior.

Current draft packages:

- [DES-01 key-rotation recipient review](references/DES-01-key-rotation-review.md) — Donut component mapping and state contract drafted; screenshot/interaction evidence and product-owner approval remain pending. This draft does not authorize production UI implementation.
- [DES-01 workspace switch and unified head table](references/DES-01-workspace-switch-head-table.md) — Unified search, faceted filter, multi-level sort, and workspace switcher layout conforming to Donut upstream and Lucide icon tokens.

## 2. Approved reference checklist

### Product and authority

- [ ] Screen has one named primary job.
- [ ] Organization/workspace/account/platform-admin context is unmistakable.
- [ ] Ownership, effective access, runtime, lease, sync, and freshness appear where relevant.
- [ ] Primary action maps to a real capability and endpoint/command.
- [ ] Destructive action names scope, consequence, reversibility, and audit behavior.
- [ ] Unsupported capability is explained from real runtime/policy data.
- [ ] Share, clone, and export remain distinct.
- [ ] No UI-only permission assumption.

### Layout and hierarchy

- [ ] Correct shell mode used.
- [ ] Page title, context, actions, filters, content, and inspector follow consistent order.
- [ ] Dense content uses tables/lists rather than decorative cards.
- [ ] Main action is visually dominant once, not repeated in every section.
- [ ] Long labels, IDs, translations, and maximum practical rows do not break layout.
- [ ] No nested cards without independently actionable inner resource.
- [ ] Operational radii do not exceed 12px.
- [ ] Empty space supports hierarchy rather than simulating a marketing page.

### Components

- [ ] Controls use defined heights, typography, spacing, border, and focus tokens.
- [ ] Every icon has consistent family/weight and accessible name when needed.
- [ ] Status combines text/icon/shape, not color alone.
- [ ] Disabled actions expose a reason when safe.
- [ ] Bulk action bar shows selected count and clear action.
- [ ] Dialog/drawer choice matches task complexity.
- [ ] Durable operations create jobs and survive dismissal.
- [ ] Secrets are masked and excluded from accidental copy/export.

### States

- [ ] Required state-matrix cells have references or documented non-applicability.
- [ ] Loading preserves known shell/context and does not mimic final data inaccurately.
- [ ] First-use empty differs from filtered empty.
- [ ] Errors name failed operation, retained work, retry/recovery, and correlation details.
- [ ] Forbidden state does not leak protected records.
- [ ] Offline state preserves valid local work.
- [ ] Degraded state names stale/missing data and freshness.
- [ ] Lock/conflict states provide safe default and escalation path.
- [ ] Partial completion reports succeeded, failed, skipped, unchanged, retryable.
- [ ] Success appears only after authority confirms it.

### Accessibility

- [ ] Entire flow completes by keyboard.
- [ ] Focus order matches visual/task order.
- [ ] Focus is visible on every interactive element.
- [ ] Dialog focus is trapped and returns correctly.
- [ ] Form labels, instructions, errors, and summaries are programmatically associated.
- [ ] Table headers, sort state, selection count, and row actions are announced.
- [ ] Live regions announce async changes without repetition.
- [ ] Touch targets meet 44×44px on touch layouts.
- [ ] 200% zoom and 320 CSS px reflow preserve essential public/account functions.
- [ ] Reduced-motion reference removes spatial animation.
- [ ] Contrast passes WCAG 2.2 AA; non-text controls/focus pass 3:1.

### Content and anti-slop

- [ ] Labels are verb-first and domain-specific.
- [ ] Error copy states condition, cause, next action.
- [ ] No `Submit`, vague `OK`, or unexplained `Something went wrong`.
- [ ] No fake records, dead actions, generic KPI filler, invented metrics, or testimonials.
- [ ] No gradients, glow, glass, floating blobs, sparkle icons, or decorative AI motifs.
- [ ] No giant operational headings or default three-card dashboard rows.
- [ ] No excessive pills, pastel confetti, shadows, or symmetrical card mosaics.
- [ ] Motion is functional and under 320ms.

### Responsive

- [ ] Primary job remains reachable at every required viewport.
- [ ] Sidebar/drawer transition is defined.
- [ ] Tables define column priority, structured-list fallback, or justified horizontal scroll.
- [ ] Filters become a sheet with visible applied-filter summary.
- [ ] Bulk selection becomes a sticky touch action bar.
- [ ] Dialogs become full-height sheets where required.
- [ ] Security/ownership/lease/error state is never hidden for space.
- [ ] Hover-only actions have touch and keyboard alternatives.

## 3. Screenshot viewport matrix

Screenshots use CSS pixels at 100% zoom unless a zoom/accessibility case is named. Capture standard and worst-case data fixtures.

| ID | Viewport | Surface | Required use |
|---|---:|---|---|
| V01 | 320×800 | Public/account | Minimum reflow and narrow mobile |
| V02 | 390×844 | Public/account/workspace | Common mobile portrait |
| V03 | 430×932 | Public/account/workspace | Large mobile portrait |
| V04 | 768×1024 | All web | Tablet portrait; sidebar-to-drawer behavior |
| V05 | 1024×768 | Desktop app/all web | Minimum desktop operations viewport |
| V06 | 1280×800 | All | Compact laptop baseline |
| V07 | 1440×900 | All | Primary approval viewport |
| V08 | 1536×864 | Desktop/workspace/admin | Common desktop scale |
| V09 | 1920×1080 | Desktop/workspace/admin/public | Wide layout and max-width behavior |
| V10 | 2560×1440 | Desktop/workspace/admin | Ultrawide containment and density |
| V11 | 1280×800 at 200% zoom | Public/account | WCAG zoom behavior |
| V12 | 1440×900 reduced motion | All | Motion fallback |
| V13 | 1440×900 high contrast/forced colors | Web/Desktop where supported | Control and focus legibility |

### Required viewport assignments

| Surface/screen type | Required viewports |
|---|---|
| Public landing/product/security | V01, V04, V07, V09, V11, V12 |
| Download/docs/status | V01, V04, V07, V09, V11 |
| Sign-in/MFA/recovery/invite | V01, V04, V07, V11, V13 |
| Account settings/tokens/sessions | V01, V04, V07, V11, V13 |
| Workspace inventories/details | V02, V04, V05, V07, V09, V13 |
| Workspace settings/policies | V02, V04, V05, V07, V09 |
| Desktop profile/resource screens | V05, V06, V07, V09, V10, V12 |
| Desktop diagnostics/logs/jobs | V05, V07, V09, V10, V13 |
| Migration flow | V05, V07, V09, V10, V12, V13 |
| Platform admin | V05, V07, V09, V10, V13 |

Mobile workspace screens are approval-required even if launch scope later limits mutations; unsupported actions must be intentionally represented, not accidentally broken.

## 4. Screenshot scene matrix

Every P0 screen captures these scenes when applicable:

| Scene | Fixture |
|---|---|
| Typical | Realistic normal data, standard role |
| Maximum | Long labels, many columns, high counts, multiple statuses |
| Empty | First-use zero data |
| Filter empty | Search/filter with zero matches |
| Loading | Initial and mutation loading |
| Recoverable error | Retryable network/service failure |
| Forbidden | Authenticated insufficient capability |
| Offline | Remote unavailable with valid local data |
| Degraded | Partial/stale data and unavailable actions |
| Locked | Lease/session/policy lock |
| Conflict | Sync/migration/access conflict |
| Partial completion | Mixed bulk/job result |
| Destructive confirm | Exact object count and consequences |
| Success | Confirmed resulting state, not decorative toast alone |

## 5. Visual diff rules

- Baselines are versioned by screen ID, viewport ID, state, theme, locale, and fixture.
- Dynamic timestamps, generated IDs, progress, and avatars use deterministic test fixtures.
- Ignore masks are limited to approved nondeterministic regions; never mask primary content or layout.
- Pixel diffs are triaged alongside semantic checks. A low diff does not prove correct permissions, copy, focus, or state.
- Threshold changes require design approval and rationale; do not raise thresholds to hide regressions.
- Capture full page only for public/editorial pages. Operational approval uses viewport screenshots plus targeted scroll positions.
- Store failure evidence with expected, actual, diff, browser/OS, viewport, commit, and fixture version.

## 6. Browser and OS coverage

Minimum web visual coverage pending engineering confirmation:

- Chromium current and previous major on Windows.
- Edge current on Windows.
- Safari current and previous major on macOS/iOS.
- Firefox current on desktop.

Desktop app visual coverage pending supported-OS decision:

- Windows 11 at 100%, 125%, and 150% display scaling.
- macOS current and previous major on standard and Retina scaling.
- Linux only if promoted to first-class support; name distribution/window manager baseline.

## 7. Approval record template

```text
Screen ID:
Reference version:
Surface/shell:
Role and capabilities:
Fixture version:
State coverage:
Viewport coverage:
Accessibility review:
Content review:
Design review:
Engineering feasibility review:
Security/permission review:
Approved by:
Approval date:
Known deviations:
Re-review trigger:
```

## 8. Re-review triggers

Repeat approved-reference review when:

- Domain status or capability enums change.
- Permission, lease, sync, migration, token, or destructive semantics change.
- A new runtime introduces different capabilities.
- A component token changes globally.
- A P0 screen gains or loses a primary action.
- Responsive behavior changes at a required viewport.
- Localization expands labels by more than 30%.
- Browser/OS rendering changes produce material layout drift.
- A screenshot threshold or ignored region changes.

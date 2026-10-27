# BugLogin 2.0 Design System

Status: implementation-ready baseline; Donut-compatible authority locked  
Owner: Product Design  
Last updated: 2026-08-11  
Applies to: desktop application, public web, account shell, workspace shell, platform admin shell

## Binding visual authority

For organization/workspace management, account provisioning and editing (single/bulk), resource access, Platform Admin and personal account flows, use the [canonical management UI/UX specification (2026-10-26)](docs/superpowers/specs/2026-10-26-management-ui-ux-design.md). It replaces conflicting earlier management ASCII/layout decisions while preserving the shared Donut visual system and backend permission authority.

Desktop visual and interaction behavior starts from the pinned Donut upstream source and its semantic theme mechanism. BugLogin-specific screens extend that language; they do not replace it with an agent-created CSS system. Web shells may change density and responsive composition by mode, but they share the same typography, semantic color roles, control geometry, status taxonomy, icon rules, and accessibility behavior. See [UI authority](docs/design/ui-authority.md).

## 1. Product design principles

BugLogin is an operations product for launching, governing, sharing, migrating, and diagnosing browser profiles and related resources. The interface must feel dependable under pressure: dense without feeling cramped, explicit about authority and state, fast for repeated work, and calm when systems are degraded.

1. **Authority is visible.** Every resource view identifies organization, workspace, owner, effective access, lease state, runtime, and sync health where relevant.
2. **Local-first truth is legible.** Desktop differentiates local, queued, syncing, synced, conflicting, unavailable, and policy-blocked states.
3. **Operations beat presentation.** Tables, command bars, filters, split panes, logs, and progress surfaces are first-class. Decorative cards are exceptional.
4. **Danger is proportional.** Reversible actions stay lightweight. Destructive, security-sensitive, or externally visible actions show scope, consequences, and recovery path.
5. **Progressive disclosure preserves speed.** Common paths remain compact. Advanced fingerprint, runtime, network, and policy controls open in drawers or dedicated detail sections.
6. **One system, four modes.** Public pages may breathe; account pages remain focused; workspace pages are dense; platform admin pages are austere and audit-heavy.
7. **No fake parity.** Runtime and integration capabilities come from real capability data. Unsupported controls are explained, not silently hidden.
8. **Every screen survives failure.** Loading, empty, error, forbidden, offline/degraded, partial, locked, and success states are designed before implementation.

## 2. Visual direction

### 2.1 Character

**Donut-compatible operational:** compact technical controls, upstream-aligned rail/toolbar/table/drawer behavior, sharp hierarchy, semantic color used only for meaning, and no consumer-SaaS decoration. Desktop must remain recognizably compatible with Donut while BugLogin adds workspace, sync, ACL, migration, and diagnostic context. Public web may use more whitespace and product evidence without becoming a separate brand.

Design adjectives: precise, quiet, durable, accountable, local-first, technical.

Avoid glossy glass, neon gradients, giant rounded cards, oversized headings, floating blobs, excessive badges, playful illustrations in operational flows, and dashboard mosaics without user jobs.

### 2.2 Surface modes

| Mode | Primary use | Density | Canvas | Navigation |
|---|---|---:|---|---|
| Desktop operations | Profiles, resources, jobs, diagnostics | Compact | `canvas-subtle` | App rail + workspace sidebar + command bar |
| Public web | Product, security, docs, download | Comfortable | `canvas-default` | Slim public header |
| Account | Sign-in, sessions, preferences, tokens | Comfortable | `canvas-default` | Narrow account header/sidebar |
| Workspace | Members, teams, resources, audit, billing | Compact | `canvas-subtle` | Org/workspace switcher + sidebar |
| Platform admin | Support, policy, incidents, audit | Compact | `canvas-admin` | Distinct admin masthead + sidebar |

Theme behavior follows the pinned Donut theme provider and semantic variables. Light, dark, system, and approved custom themes may exist only through that mechanism or a typed BugLogin mapping; no screen may hardcode a competing page-wide palette.

### 2.3 Donut compatibility boundary

- Reuse upstream components and patterns before adding new primitives.
- Preserve compact profile/group toolbar, table, status, drawer, command palette, keyboard, and focus behavior.
- Map BugLogin tokens to upstream semantic variables; do not duplicate color, spacing, radius, or icon scales in raw CSS.
- A visual deviation requires a screen reference, rationale, owner, and screenshot review before implementation.

## 3. Design tokens

Token names are normative. Values may be translated into CSS variables, theme objects, or native desktop tokens without renaming semantic intent.

### 3.1 Color

#### Neutral foundations

| Token | Value | Usage |
|---|---|---|
| `color.canvas.default` | `#FCFCFB` | Public/account page canvas |
| `color.canvas.subtle` | `#F5F6F7` | Desktop/workspace operations canvas |
| `color.canvas.admin` | `#F2F3F5` | Platform admin canvas |
| `color.surface.default` | `#FFFFFF` | Primary panels, menus, dialogs |
| `color.surface.subtle` | `#F8F9FA` | Secondary sections, table headers |
| `color.surface.sunken` | `#EEF0F2` | Wells, code/log backgrounds, disabled tracks |
| `color.surface.selected` | `#EAF2FF` | Selected rows/items |
| `color.border.subtle` | `#E4E7EB` | Internal dividers |
| `color.border.default` | `#D3D8DE` | Inputs, panels, table outlines |
| `color.border.strong` | `#AEB6C0` | Emphasized boundaries |
| `color.text.primary` | `#17202A` | Primary copy |
| `color.text.secondary` | `#4F5B67` | Supporting copy |
| `color.text.muted` | `#74808C` | Metadata, placeholders |
| `color.text.disabled` | `#9DA6AF` | Disabled content only |
| `color.icon.default` | `#53606C` | Neutral icons |
| `color.overlay` | `rgba(15, 23, 32, 0.48)` | Modal scrim |

#### Brand and interaction

| Token | Value | Usage |
|---|---|---|
| `color.brand.700` | `#174EA6` | Pressed primary, strong links |
| `color.brand.600` | `#1F64D1` | Primary actions, active nav, focus |
| `color.brand.500` | `#3478E5` | Hover accents, charts |
| `color.brand.100` | `#DCEAFF` | Selected/active backgrounds |
| `color.focus.ring` | `#2E74D6` | 2px keyboard focus ring |

#### Semantic

| Meaning | Strong | Text | Soft background | Border |
|---|---|---|---|---|
| Success | `#238636` | `#176B2C` | `#E9F7ED` | `#A9D9B4` |
| Warning | `#B66A00` | `#8A4F00` | `#FFF4DE` | `#E8C17A` |
| Danger | `#C73535` | `#9F2525` | `#FDECEC` | `#E5AAAA` |
| Info | `#1F64D1` | `#174EA6` | `#EAF2FF` | `#AFCBF5` |
| Neutral | `#697580` | `#53606C` | `#EEF0F2` | `#CDD2D8` |

Rules:

- Semantic color never stands alone; pair it with text, icon, shape, or status label.
- Reserve red for destructive action, active failure, revoked access, or critical security state.
- Reserve green for completed or healthy state, not primary actions.
- Charts use a color-blind-safe sequence: blue `#3478E5`, teal `#158B8B`, amber `#C77B17`, violet `#7857C7`, rose `#C94F74`, slate `#697580`.
- Long text on colored backgrounds must meet WCAG 2.2 AA contrast. Controls and focus indicators must meet 3:1 against adjacent colors.

### 3.2 Typography

Font strategy favors bundled or self-hosted open fonts to keep desktop startup and public performance predictable.

| Role | Family | Weight | Notes |
|---|---|---:|---|
| Product/UI | `Geist Sans`, fallback `Segoe UI`, sans-serif | 400–650 | Compact, neutral, strong numerals |
| Display/public | `Geist Sans` | 550–700 | Same family prevents brand/UI split |
| Data/code | `Geist Mono`, fallback `Cascadia Mono`, monospace | 400–600 | IDs, logs, IPs, ports, hashes, timestamps |

Use tabular numerals for counts, durations, quotas, timestamps, IP/port columns, and financial data.

| Token | Size / line-height | Weight | Usage |
|---|---|---:|---|
| `type.display.lg` | 48/52 | 650 | Public hero only; max 2 lines |
| `type.display.md` | 38/44 | 650 | Public section lead |
| `type.heading.xl` | 28/34 | 650 | Spacious/public screen title |
| `type.heading.lg` | 22/28 | 650 | Operational screen title |
| `type.heading.md` | 18/24 | 620 | Panel/dialog title |
| `type.heading.sm` | 15/20 | 620 | Section/table group title |
| `type.body.lg` | 16/24 | 400 | Public lead, account explanations |
| `type.body.md` | 14/20 | 400 | Default UI text |
| `type.body.sm` | 13/18 | 400 | Dense controls and table cells |
| `type.label` | 12/16 | 600 | Field labels, column headers |
| `type.caption` | 11/16 | 500 | Timestamps, compact metadata |
| `type.code` | 12/18 | 450 | Logs, IDs, snippets |

Rules:

- Never use all-caps for sentences. Uppercase is limited to short environment labels such as `PRODUCTION`.
- Operational headings use sentence case.
- Public body measure: 60–72 characters. Operational explanatory text: 45–68 characters.
- Never shrink essential text below 12px. Tooltips and legal annotations may use 11px with adequate contrast.

### 3.3 Spacing

Base unit: 4px. Compact density comes from smaller gaps and control heights, not smaller touch targets on touch devices.

| Token | Value |
|---|---:|
| `space.0` | 0 |
| `space.0_5` | 2px |
| `space.1` | 4px |
| `space.1_5` | 6px |
| `space.2` | 8px |
| `space.3` | 12px |
| `space.4` | 16px |
| `space.5` | 20px |
| `space.6` | 24px |
| `space.8` | 32px |
| `space.10` | 40px |
| `space.12` | 48px |
| `space.16` | 64px |
| `space.20` | 80px |
| `space.24` | 96px |

Operational defaults: page inset 20–24px, panel padding 16px, form row gap 16px, toolbar gap 8px, table cell horizontal padding 12px, table cell vertical padding 8px.

Public defaults: page inset 24–64px by breakpoint, section spacing 80–128px, content block gap 24–40px.

### 3.4 Shape, borders, elevation

| Token | Value | Usage |
|---|---:|---|
| `radius.xs` | 3px | Tags, compact controls |
| `radius.sm` | 5px | Buttons, inputs, table selections |
| `radius.md` | 8px | Menus, popovers, panels |
| `radius.lg` | 12px | Dialogs, public media |
| `radius.full` | 999px | Avatars, status dots only |
| `border.hairline` | 1px | Standard structural border |
| `shadow.popover` | `0 8px 24px rgba(23,32,42,.12), 0 1px 3px rgba(23,32,42,.10)` | Menus/popovers |
| `shadow.dialog` | `0 20px 60px rgba(23,32,42,.20), 0 2px 8px rgba(23,32,42,.10)` | Modal dialogs |

Do not elevate ordinary panels. Prefer borders, dividers, and canvas contrast. Nested cards are prohibited unless the inner element is an independently actionable resource.

### 3.5 Iconography

- Use one line-icon family with 1.5px optical stroke, square 16/20/24px sizes, and filled variants only for selected state or severity.
- Icons supplement labels; unfamiliar actions require text.
- Product concepts need stable icons: profile, runtime, proxy, VPN, cookie set, extension, team, member, workspace, organization, token, audit, job, sync, backup.
- Never use emoji, decorative sparkle icons, or mixed icon libraries.

## 4. Layout and responsive system

### 4.1 Breakpoints

| Name | Range | Primary behavior |
|---|---|---|
| `compact` | 0–599px | Single column, bottom-sheet filters, essential actions only |
| `mobile-wide` | 600–767px | Single column, wider forms, optional two-up metadata |
| `tablet` | 768–1023px | Collapsible sidebar, tables may become structured lists |
| `desktop` | 1024–1439px | Full operations shell, 12-column content grid |
| `wide` | 1440–1919px | Persistent secondary pane where valuable |
| `ultrawide` | 1920px+ | Content max-width; avoid uncontrolled line lengths |

### 4.2 Grid

- Public: 12 columns; max content width 1280px; readable content width 720px.
- Account: 12 columns; max width 1120px; forms 420–560px.
- Workspace/admin: fluid 12 columns; max operational width 1600px; tables may use full width.
- Desktop app: shell fills window. Main content minimum 640px. Optional inspector 320–420px. Sidebar 232px expanded, 56px collapsed.
- Minimum supported desktop app viewport: 1024×720. Recommended: 1440×900.
- Dense tables preserve columns by priority. Horizontal scroll is acceptable for audit, migration, and technical inventories when the first identifier column remains sticky.

### 4.3 Responsive rules

1. Preserve the primary job, not the desktop composition.
2. Sidebars become drawers below 1024px.
3. Multi-select table workflows become select mode with a sticky bottom action bar below 768px.
4. Split panes become stacked route sections or full-screen detail views below 1024px.
5. Dialogs wider than 560px become full-height sheets below 768px.
6. Hover-only disclosure is prohibited; every action remains keyboard and touch reachable.
7. Filters collapse into a sheet; applied filters remain visible as removable text chips.
8. Do not hide security, ownership, lease, or failure state to save space.

## 5. Shells

### 5.1 Desktop operations shell

Structure:

1. Native title/drag region where required.
2. 56px app rail: Profiles, Resources, Jobs, Diagnostics, Settings.
3. 232px contextual sidebar: workspace selector, saved views, groups, resource categories.
4. Main page: breadcrumb/context line, title/action row, optional filter/command bar, content.
5. Optional inspector: selected item details, activity, permissions, diagnostics.
6. Global status strip only when offline, sync degraded, update required, or policy state affects multiple screens.

Behavior:

- `Ctrl/Cmd+K` opens command/search palette.
- `Ctrl/Cmd+N` invokes context-aware create action.
- Workspace and environment changes are explicit and persisted.
- Global jobs remain reachable without modal blocking.
- Profile launch is never visually conflated with opening profile details.

### 5.2 Public web shell

- Header: logo, Product, Security, Download, Docs, Sign in; one primary action maximum.
- Hero: product job plus concrete interface proof; no abstract AI claim.
- Main content alternates editorial explanation with real product screenshots or diagrams.
- Footer: product, resources, legal, status, security contact, release channel.
- Download detects OS but always exposes all supported builds and checksums.

### 5.3 Account shell

- Narrow header with product mark, workspace return link, help, account menu.
- Sidebar: Profile, Security, Sessions, API/MCP tokens, Notifications, Connected apps.
- Security pages prioritize current risk and recent activity over promotional copy.
- Destructive account actions live in a separated danger section with re-authentication.

### 5.4 Workspace shell

- Header: organization/workspace switcher, global search, jobs/activity, help, user menu.
- Sidebar groups: Overview; Profiles and resources; People; Access; Activity; Settings.
- Sidebar labels reflect domain nouns, not implementation modules.
- Cross-workspace action requires an explicit destination and confirmation.

### 5.5 Platform admin shell

- Visually distinct `Platform admin` masthead and environment label.
- No customer-facing branding flourishes.
- Every mutation exposes reason field, affected scope, audit event, and rollback/recovery path when available.
- Impersonation uses a persistent banner, elapsed time, reason, and one-step exit.
- Platform admin roles never masquerade as workspace roles.

## 6. Core components

### 6.1 Buttons

| Variant | Usage | Height desktop | Height touch |
|---|---|---:|---:|
| Primary | One dominant action per region | 32px | 44px |
| Secondary | Normal actions | 32px | 44px |
| Quiet | Toolbars, row actions | 28–32px | 44px |
| Danger | Confirmed destructive action | 32px | 44px |
| Split | Primary action with closely related options | 32px | 44px |
| Icon | Familiar action with accessible name | 28–32px | 44px |

Buttons use verb-first labels: `Create profile`, `Retry sync`, `Revoke access`. Avoid `Submit`, `Yes`, and `OK` when a specific action exists. Loading buttons preserve width and label context.

### 6.2 Forms

- Labels sit above controls. Required status appears in validation copy or concise marker with legend.
- Default control height: 34px desktop, 44px touch. Textarea minimum 96px.
- Help text precedes errors; errors appear below the field and in a summary for long forms.
- Validate on blur or submit. Do not interrupt typing for format errors unless input is impossible.
- Network/proxy parsers show accepted examples, parsed preview, duplicate count, and row-level failures before commit.
- Advanced settings use named sections, not an undifferentiated accordion pile.
- Destructive forms require explicit scope; typed confirmation only for high-impact irreversible operations.

### 6.3 Select, combobox, search

- Select: fewer than 12 stable options.
- Combobox: large, searchable, or remote datasets.
- Search: debounced, cancelable, with query retained on navigation back.
- Option rows can include status and metadata but must preserve a clear primary label.
- Empty search distinguishes no match from no available records.

### 6.4 Tables and resource lists

Tables are the default for comparable operational records. Cards are reserved for launch-oriented profile grids, public content, and small mixed-shape summaries.

Required table anatomy:

- Sticky header for long tables.
- Checkbox selection only when bulk actions exist.
- Primary identifier in first data column; row click opens details only when row contains no ambiguous controls.
- Sort state in header, including accessible announcement.
- Filter bar with result count and clear-all.
- Column visibility and density controls for inventories with more than eight useful fields.
- Pagination for stable server lists; virtualization or cursor loading for logs/events.
- Row state may use a 3px leading indicator plus text status; never background color alone.
- Row actions: maximum three visible; remainder in overflow menu.

Default density: 36px rows. Comfortable option: 44px. Technical logs: 30–32px where no touch use is expected.

### 6.5 Profile row/card

Required content:

- Name and profile ID suffix.
- Runtime and version.
- Owner/workspace.
- Proxy/network summary.
- Local/sync state.
- Lease/lock state when shared.
- Last opened or modified time.
- Primary `Launch` action with disabled reason when unavailable.

Never show fingerprint claims as decorative scores. Show actual configured attributes or policy status in details.

### 6.6 Status badge

Badges use short nouns/adjectives: `Running`, `Queued`, `Synced`, `Conflict`, `Revoked`. Maximum two badges in a row; additional state becomes metadata or a status summary. Status taxonomy must map to backend enums and documented user language.

### 6.7 Command and filter bars

- Command bar aligns page actions, view switch, density, refresh, and overflow.
- Filter bar shows search, high-value filters, saved view, applied count.
- Bulk action bar replaces the standard bar while selection is active and includes selected count plus `Clear selection`.
- Filters commit immediately when reversible; expensive queries use explicit `Apply filters`.

### 6.8 Tabs

Use tabs for peer views of one resource, not workflow steps. Maximum six visible tabs. Overflow becomes `More`. Tabs preserve URL/state on web and stable local route state on desktop.

### 6.9 Panels, inspectors, drawers

- Panel: persistent grouping on canvas.
- Inspector: selected-record detail alongside a list; does not replace full edit routes.
- Drawer: contextual create/edit/detail that preserves list context.
- Avoid drawers inside drawers. A drawer may open one confirmation dialog only.
- Unsaved changes require clear save/discard behavior; never silently dismiss.

### 6.10 Dialogs

Use dialogs for short, interruptive decisions. Use a route or drawer for multi-section work.

- Title states action and object.
- Body begins with consequence, then details.
- Footer order: cancel/secondary first; primary/danger last.
- Destructive dialog names object count, downstream effects, reversibility, and audit behavior.
- Initial focus targets the safest meaningful control. Focus returns to invoker.

### 6.11 Toasts, banners, inline feedback

- Toast: completed background event with no required decision; persists 5–8 seconds and enters notification history if important.
- Inline alert: local problem the user can fix in context.
- Banner: page- or system-wide condition.
- Modal: required immediate decision only.
- Success should usually update the page rather than create a congratulatory screen.

### 6.12 Empty states

Every empty state answers what is absent, why it may be absent, and the valid next action. First-use empties may include one compact diagram. Filter empties preserve data context and offer `Clear filters`. Permission empties are prohibited; use a forbidden state.

### 6.13 Jobs and progress

- Long operations create a durable job with state, progress, start time, actor, affected objects, logs, cancel/retry where safe.
- Progress bars show determinate progress only from real measurements.
- Unknown duration uses activity indicator plus current stage text.
- Partial completion lists succeeded, failed, skipped, and rollback state.
- Closing a dialog never cancels a durable job unless stated explicitly.

### 6.14 Audit timeline

Each event exposes time, actor, action, object, scope, result, source, correlation ID, and reason where required. Human summary appears first; technical payload is expandable and copyable. Timestamps default to user locale with exact UTC available.

### 6.15 Code, logs, identifiers

- Monospace, selectable, copy action, no forced wrapping for hashes/IPs.
- Secret values are masked by default and excluded from copy unless explicitly revealed with permission.
- Log severity combines icon, label, and color.
- Search matches are highlighted accessibly; auto-scroll may be paused.

## 7. Interaction patterns

### 7.1 Create profile

Use a three-stage route or large drawer:

1. **Basics:** name, workspace/group, runtime/template.
2. **Network and identity:** proxy parse/selection, locale/timezone, fingerprint policy summary.
3. **Review:** capability compatibility, storage/sync policy, validation, create-and-launch option.

Common defaults are preselected from workspace policy. Advanced engine-specific settings remain collapsed but summarize overridden values. Bulk create uses import/preview rather than repeating the single form.

### 7.2 Share, clone, export

These are separate actions and dialogs.

- Share: grantee, capabilities, expiry, concurrency/lease behavior.
- Clone: destination, copied data categories, new ownership, runtime compatibility.
- Export: archive contents, secret/cookie inclusion, encryption, expiry, audit warning.

Never label all three as `Share`.

### 7.3 Lease and conflict

- Active lease is visible in list and detail.
- Launch conflict explains holder, heartbeat recency, allowed actions, and escalation policy.
- Force release is privileged, reason-required, audited, and warns about data loss.
- Sync conflict presents both versions, affected categories, safe default, and recovery copy before overwrite.

### 7.4 Migration

Migration is a dedicated guided workspace, not a generic import dialog:

1. Inspect source.
2. Snapshot and verify immutability.
3. Map entities and resolve exceptions.
4. Dry run.
5. Review report.
6. Execute or cut over.
7. Verify and retain rollback evidence.

The UI never suggests success from record counts alone. Show hashes, launch tests, permission reconciliation, unsupported profiles, and residual risk.

### 7.5 Permission and policy explanation

Disabled actions expose one concise reason and a route to resolution when safe: `Requires profile.edit_metadata`, `Unavailable while leased by …`, `Runtime does not support cookie export`, `Workspace policy blocks external sharing`.

Do not reveal confidential policy details to unauthorized users.

## 8. Accessibility

Target: WCAG 2.2 AA for web and equivalent keyboard/screen-reader quality for desktop webviews.

- Full keyboard operation; no keyboard traps outside intentional modal focus containment.
- Visible 2px focus ring with 2px offset; focus is never removed.
- Touch targets minimum 44×44px on touch layouts; adjacent destructive targets separated.
- Landmark structure: header, nav, main, complementary inspector, footer where present.
- One `h1` per screen; heading levels follow visual hierarchy.
- Tables use real headers, scopes, captions/accessible names, and announced sort state.
- Live regions announce async completion, validation summaries, job state changes, and selected counts without excessive chatter.
- Drag/drop always has menu/button alternatives.
- Color, position, and animation never carry meaning alone.
- Zoom to 200% and reflow to 320 CSS px without loss of essential function on public/account pages. Dense operational tables may scroll horizontally with sticky context.
- Respect `prefers-reduced-motion`; pause non-essential animation. No flashing content.
- Authentication supports password managers, paste, accessible MFA, recovery codes, and clear session timeout warnings.
- Language identifies objects and outcomes; avoid vague `Something went wrong` where an actionable category is known.

## 9. Motion

Motion is minimal-functional.

| Token | Duration | Usage |
|---|---:|---|
| `motion.instant` | 0–80ms | Press/selection feedback |
| `motion.fast` | 120ms | Hover, menu highlight, small disclosure |
| `motion.base` | 180ms | Popover, drawer micro-transition |
| `motion.slow` | 240ms | Dialog, full inspector transition |
| `motion.max` | 320ms | Rare page-level spatial transition |

Easing:

- Enter: `cubic-bezier(0.16, 1, 0.3, 1)`
- Exit: `cubic-bezier(0.4, 0, 1, 1)`
- Move: `cubic-bezier(0.2, 0, 0, 1)`

Rules:

- Animate opacity and transform only where possible.
- No spring/bounce on operational controls.
- Table sorting/filtering must not animate rows through long distances.
- Progress animation cannot imply measured completion.
- Reduced motion removes spatial movement while preserving state feedback.
- Public web may use subtle screenshot reveals; no scroll hijacking, pinned storytelling, or ambient blobs.

## 10. Content design

- Use direct verbs and domain nouns: `Launch profile`, `Assign team`, `Revoke token`.
- State first, cause second, action last: `Sync paused. Storage is unreachable. Retry when connectivity returns.`
- Distinguish delete, revoke, remove, unlink, stop, cancel, and archive.
- Preserve technical details behind `Details` where they help support without overwhelming normal users.
- Dates use locale format; relative time includes exact timestamp on hover/focus.
- Counts use explicit units: `12 profiles`, `480 MB`, `3 active sessions`.
- Never call security-sensitive bypasses magic, smart, or one click.

## 11. Anti-AI-slop rules

Reject any screen containing these patterns without a job-specific rationale:

1. Gradient hero with floating generic product cards.
2. Huge headline consuming half the viewport in an operations shell.
3. Four identical KPI cards as default page content.
4. Card-inside-card nesting for ordinary sections.
5. Excessive pills, rounded rectangles, or pastel status confetti.
6. Generic copy: `Supercharge`, `Unlock`, `Seamless`, `Powerful`, `Revolutionize`, `AI-powered` without evidence.
7. Sparkles, robot heads, abstract orbs, fake terminal text, fake charts, or invented testimonials.
8. Every section centered; every layout symmetric; every action in a card.
9. Decorative motion, parallax, scroll hijacking, or looping operational animation.
10. Hidden labels replaced by placeholder-only forms.
11. Disabled controls without reason.
12. Fake records, lorem ipsum, dead primary actions, or optimistic success before authority confirms it.
13. Generic recent activity used as filler when a real audit/job view exists.
14. Mixed icon weights, arbitrary colors, shadows on all surfaces, or radii above 12px in operations UI.
15. Mobile designs that merely shrink desktop tables.

## 12. Design QA and governance

- `DESIGN.md` is the normative design source. Supporting implementation matrices live in `docs/design/`.
- New components require a repeated product need or an established platform primitive gap.
- Variants must map to semantic state or density, not visual novelty.
- Any token exception needs documented reason and approval.
- Screen implementation starts only after its IA row, state matrix, and approved-reference checklist are complete.
- Design review evaluates real data, longest labels, least-privileged roles, degraded network, and destructive paths.

Required supporting documents:

- [Information architecture and screen inventory](docs/design/information-architecture.md)
- [Per-screen state matrix](docs/design/screen-state-matrix.md)
- [Approved-reference and screenshot checklist](docs/design/approved-reference-checklist.md)

## 13. Open assumptions

These assumptions require product/engineering confirmation before visual implementation locks:

- Supported desktop OS set and whether Linux is first-class.
- Whether dark mode is launch scope.
- Primary user persona and top three repeated jobs.
- Final public brand assets, product wordmark, legal product name, and favicon.
- Final runtime names, capability taxonomy, and legacy deprecation labels.
- Billing scope, plan names, quotas, and checkout ownership.
- Exact role/capability model and which platform-admin actions permit impersonation.
- Supported locales, localization launch scope, and timezone defaults.
- Mobile operational scope: full mutation support versus monitoring/approval subset.
- Required compliance claims and approved security language.

## 14. Decision log

| Date | Decision | Rationale |
|---|---|---|
| 2026-07-20 | Operational editorial visual direction | Supports dense, high-consequence work without consumer-SaaS decoration |
| 2026-07-20 | Geist Sans and Geist Mono | Compact open font family with clear technical data and consistent public/product voice |
| 2026-07-20 | Light-first warm-neutral palette | Improves long-session legibility and keeps semantic state colors scarce |
| 2026-07-20 | Four explicit shell modes | Prevents public, account, workspace, and platform-admin concerns from collapsing into one generic dashboard |
| 2026-07-20 | 4px spacing base and compact operations density | Matches desktop operations needs while preserving accessible touch adaptations |
| 2026-07-20 | Minimal-functional motion | Preserves state clarity, performance, and reduced-motion accessibility |

# Resource Management Rebuild Design

Status: implementation in progress  
Date: 2026-10-26  
Owner: IO-UI, using existing IO-CONTROL/API contracts

## Goal

Replace the fragmented organization, workspace, group/profile, account/member, and platform administration experience with one coherent, API-backed operational hierarchy. Preserve Donut desktop shell, semantic tokens, real backend authorization, and existing API meanings. This is a UI and workflow rebuild; it does not claim new server capabilities.

## Canonical hierarchy and routes

```text
Platform Admin (global; separate authority)
├── Organizations → Organization detail → Workspaces
├── Accounts / identities
├── Resources / grants
└── Audit (only where a real API is available)

Workspace context (regular app)
Organization → Workspace → [Profiles | Groups | Members | Resources]
                                  ├── Group → Profiles / Shares
                                  ├── Profile → lifecycle / sync / access
                                  └── Member → role / status / grants
```

There is one context switcher. The active Organization, Workspace, and API-derived role are shown once. Workspace navigation shows only sections; it does not repeat quota/role metrics. Platform Admin is a distinct scope and may not be inferred from an Owner badge.

## Domain rules that UI must preserve

- Organization owns workspaces. Current contracts do not create an organization-wide membership or suspend operation.
- Account provisioning and invitation are different workflows and use their existing scoped APIs.
- Member role and active/suspended state belong to a workspace membership.
- Groups and profiles are workspace scoped. Group membership/shares do not imply cross-workspace profile transfer.
- Role labels do not grant authority. Server policy is authoritative; frontend checks only improve discoverability.
- Show only metrics returned by a real API. Never ship ASCII example counts, fake health, or inferred role/quota values.
- A mutation must surface API failure and refresh the affected in-memory list without requiring F5.

## Interaction model

- Lists use the existing Donut-compatible table, compact search/filter/selection toolbar, sticky actions, and internal scroll region.
- Selecting a row opens a linkable detail route or a shared inspector. Creation/edit flows use a single reusable right-side form panel for non-destructive operations; destructive and security-sensitive actions require explicit confirmation. No nested dialogs and no full-page form embedded in a legacy modal.
- Org, Workspace, Group, Profile, Member, Account, and Platform resources each have a canonical list/detail entry point. Context and selected entity survive back/forward navigation.
- Actions are grouped by entity and scope. Each action calls an existing typed desktop API command; unsupported operations are omitted with a clear capability explanation, never represented by dead buttons.
- All data surfaces implement loading, empty, filtered-empty, error/retry, forbidden, offline/degraded, conflict/locked, pending, and success states where applicable.
- Keyboard: Tab/Shift+Tab, Enter/Space, Escape, visible focus, arrow-key table navigation where supported, Ctrl/Cmd+K global navigation, and Ctrl/Cmd+N only when the current context has a permitted create action.
- Desktop viewport remains fixed; tables and detail panels scroll internally. Narrow windows collapse side navigation into the existing responsive pattern rather than wrapping row actions into multiple lines.

## Screen acceptance

| Surface | Required actions | Source of truth |
|---|---|---|
| Organization | list, create, rename/update, open its workspaces | typed organization API |
| Workspace | list under org, create, update policy/name, switch/open, inspect groups/profiles/members | typed workspace API |
| Group/Profile | create/rename/delete group, inspect and change supported shares, profile list/assignment and profile lifecycle | existing group/profile APIs |
| Member | list, invite, direct scoped account provision, change allowed role, activate/suspend, inspect workspace scope | member/invite/account APIs |
| Account | list and manage only operations exposed by account contract; account identity is not confused with membership | account API |
| Platform Admin | global org/workspace/account/resource operations only where platform-scoped API/policy exists; audit each mutation | platform contract; never simulate global scope by flattening current user's org list |

Platform-wide actions that lack a real API remain a backend capability gap and must not be faked. The visible screen must report the exact unavailable operation, while the requested UI routes remain implemented for supported actions.

## Visual authority

Use the pinned Donut source from `.tmp/donutbrowser-upstream` at the manifest commit. Preserve the current shell, spacing, typography, semantic theme variables, existing table/control geometry, and keyboard behavior. `DES-02` is a layout reference, not permission to invent metrics or duplicate navigation. `DES-03` ASCII data is illustrative only; its example identities, IDs, counts, health, and audit events must never be copied into runtime UI.

## Acceptance gates

- Every action is traced to a typed command/API and has loading, success, and failure handling.
- Each route respects its parent scope and server authorization.
- No legacy modal is the sole entry point for a management workflow; remove duplicate/obsolete entry points after route replacement.
- Create/update/delete in Org/Workspace/Group/Member updates current views immediately from authoritative response/refetch; no F5.
- Role matrix covers Owner/Admin/Lead/Operator/Member and Platform Admin, including backend-denied cases.
- `pnpm --dir apps/desktop typecheck`, desktop self-tests, relevant Rust API tests, and UI screenshot/interaction/a11y gates pass before acceptance or release.
- Windows/macOS release evidence remains mandatory; this design does not itself authorize promoting the existing candidate.

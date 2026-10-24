# Organization, Workspace, and Resource Management Model

Status: target product and interaction model; current API/UI boundaries are called out explicitly
Last updated: 2026-10-24
Related: [Information architecture](information-architecture.md), [UI authority](ui-authority.md), [Screen state matrix](screen-state-matrix.md)

Implementation note (2026-10-24): the native desktop/API currently model Organizations → Workspaces → workspace members/groups/profiles. There is no separate organization-membership record: organization ownership authorizes work across that organization's workspaces, while ordinary member status and suspension are workspace scoped. Existing API supports direct scoped account provisioning, invitations, workspace member actions, group CRUD/shares, profile inventories/group assignment, and resource grants. The current working tree adds a dedicated workspace page with Members, Groups and Profiles tabs plus full-page account/invite flows. Remaining limits include cross-workspace profile transfer, API-derived capability discovery, full group/profile breadcrumb/deep-link state, visual/interactivity QA and release evidence. The diagrams distinguish current contract from target experience; they are not release evidence.

This document defines the intended hierarchy, authorization boundaries, account-creation flows, and page-based management UI. It is a design contract for later implementation. Existing code must be inspected and verified before any capability is described as shipped.

## 1. Resource hierarchy

```text
PLATFORM ADMIN (global authority)
└── Organizations
    ├── ORG XX01
    │   ├── Owner identity / organization settings (not org membership)
    │   ├── Workspaces
    │   │   ├── WS XX03
    │   │   │   ├── Workspace members (role + status)
    │   │   │   ├── Groups
    │   │   │   │   ├── Group A
    │   │   │   │   │   └── Profiles
    │   │   │   │   └── Group B
    │   │   │   └── Ungrouped profiles
    │   │   └── WS XX04
    │   └── Organization settings / audit
    └── ORG XX02
```

The primary navigation path is always `Organization → Workspace → Group / Ungrouped → Profile`. Resource inventories must retain their parent scope and offer a route to the next lower level.

### Resource and identity ownership (current API contract)

```text
Account identity (login credential / email verification)
└── Workspace membership (role + active/suspended state)  [many per account]
    └── Grants (capability + action + scope; explicit allow/deny)

Organization ── owns ── Workspace
Workspace ── owns ── Group (optional parent for a profile)
Workspace ── owns ── Profile ── group_id: Group ID | none
```

Creating an account provisions an identity and a workspace membership in one scoped flow. Inviting adds a pending invitation and requires recipient acceptance; it is not account creation. Suspending a workspace member changes that membership only. There is currently no single “suspend from organization” operation. An organization page may aggregate workspace memberships for discovery, but must label each row with its workspace and must not imply an organization-wide membership record.

## 2. Role and authority model

```text
Platform Admin
├── Scope: all organizations, workspaces, identities, members, resources, grants
├── Account: create directly OR invite; manage lower roles per platform policy
└── Resource: administer all scopes; every change is audited

Organization Owner (owner identity for ORG XX01)
├── Scope: that organization and its workspaces
├── Account: create directly OR invite at a selected workspace; manage its members
├── Resource: create/manage workspaces; manage resources inside owned workspaces
└── No organization-wide membership or suspension record exists in current API

Workspace Owner / Admin
├── Scope: workspace membership and explicit grants
├── Account: create directly OR invite inside an authorized workspace per API policy
└── Resource: members, groups, profiles and grants inside that workspace

Lead / Manager
├── Scope: exact capabilities and scopes; “Lead” label alone grants nothing
├── Account: manage a lower role only if effective grants and role ceiling permit
└── Resource: manage only targets inside the authorized scope

Member
├── Scope: own active membership and explicit grants
└── Administration: none by default; only explicitly granted actions
```

### Role capability matrix (scope is always checked by the API)

```text
Role / scope                 Accounts & members                    Resources
───────────────────────────────────────────────────────────────────────────────
Platform Admin               Global create/invite/manage,           All orgs, WS,
                             subject to platform role policy        groups, profiles,
                                                                    grants, audit
Organization Owner           Create workspace in owned org;         All workspaces in
(owner identity)              create account OR invite in a           owned org; member
                             chosen workspace; manage its           operations happen
                             workspace memberships                  at that WS scope
Workspace Owner / Admin      Create account OR invite in an          Members, groups,
(authorized WS)               authorized workspace; manage           profiles, grants
                             only roles allowed by API              within that WS
Lead / Manager                Same only with effective grant,        Only exact scopes
(explicit capability+scope)   action and grantable role              returned/accepted by
                                                                    server policy
Member                        None by default; grants can add        Only granted actions
                             narrowly scoped operations             and resources
```

Role labels alone do not convey authority. Each write is evaluated as `actor + effective grants + action + target + parent scope`; explicit deny, scope containment, capability, and role ceiling are server-enforced. UI checks only control discoverability.

Role names do not grant authority by themselves. The API evaluates the actor, action, target scope, and target resource. Managing a group does not imply authority to move a profile to another workspace. Creating a lower-level role is allowed only when policy permits the actor to grant that role in the target scope.

## 3. Page-based information architecture

```text
Platform
├── Organizations list
│   └── Organization detail
│       ├── Overview
│       ├── Workspaces list → Workspace detail
│       ├── Memberships by workspace → Workspace member list
│       ├── Roles and access
│       └── Audit
└── Global accounts, resources, grants, audit, and platform settings

Organization detail
├── Workspaces list → Workspace detail
├── Memberships by workspace → Workspace member list
├── Roles and access
└── Audit

Workspace detail
├── Overview
├── Groups and profiles
│   ├── Groups list → Group detail
│   │   ├── Profiles list → Profile detail
│   │   ├── Members and access
│   │   └── Audit
│   └── Ungrouped profiles list → Profile detail
├── Members list → Member detail / access
├── Roles and access
└── Audit
```

Create account, invite, role changes, resource moves, and deactivation are dedicated routes or full-page flows. A dialog must not be the only way to reach or complete a management task. Detail routes remain linkable and browser navigation must preserve safe filters, sorting, pagination, and selected tabs.

## 4. Organization pages

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / Organizations / ORG XX01                                       │
├────────────────────────────────────────────────────────────────────────────┤
│ ORG XX01                         [Create workspace]                       │
│ Tabs: Overview | Workspaces | Memberships by WS | Access | Audit           │
├────────────────────────────────────────────────────────────────────────────┤
│ Search...  Workspace ▾  Role ▾  Status ▾   [Open workspace]               │
│                                                                            │
│ WORKSPACES                           MEMBERSHIPS BY WORKSPACE              │
│ ┌──────────────────────────────┐    ┌───────────────────────────────────┐ │
│ │ WS XX03   12 memberships    │    │ Workspace  Identity   Role       │ │
│ │ 4 groups  38 profiles        │    │ WS XX03    alice@…    Admin      │ │
│ │ [Open workspace →]           │    │ WS XX04    bob@…      Lead       │ │
│ ├──────────────────────────────┤    │ …                                │ │
│ │ WS XX04   5 memberships     │    │ [Open workspace members →]        │ │
│ │ [Open workspace →]           │    └───────────────────────────────────┘ │
│ └──────────────────────────────┘                                           │
└────────────────────────────────────────────────────────────────────────────┘
```

Organization overview may aggregate membership rows from its workspaces. Actions must navigate to the selected workspace before create/invite, role change, activate, or suspend. Until an organization-membership API exists, do not show “remove from organization” or claim one suspension revokes every workspace membership.

The summary is read-only at organization scope in this contract. Create account/invite and every membership mutation are performed after entering a workspace, so the user sees the exact destination and role scope before submitting.

## 5. Workspace pages

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / ORG XX01 / WS XX03                                             │
├────────────────────────────────────────────────────────────────────────────┤
│ WS XX03                          [Create account] [Invite member]          │
│ Tabs: Overview | Groups & profiles | Members | Roles & access | Audit      │
├────────────────────────────────────────────────────────────────────────────┤
│ Groups & profiles                                                         │
│                                                                            │
│ GROUPS                                UNGROUPED PROFILES                    │
│ ┌─────────────────────────────┐      ┌───────────────────────────────────┐ │
│ │ Group A   12 profiles       │      │ Profile 01   Stopped / Synced     │ │
│ │ [Open group →]              │      │ Profile 02   Running / Uploading  │ │
│ ├─────────────────────────────┤      │ …                                │ │
│ │ Group B    8 profiles       │      │ [Open profile →]                  │ │
│ │ [Open group →]              │      └───────────────────────────────────┘ │
│ └─────────────────────────────┘                                           │
│ [Create group]                 Ungrouped is a valid profile state           │
└────────────────────────────────────────────────────────────────────────────┘
```

Workspace membership is the current account-access unit. Account creation, invitation, role changes, activation, and suspension must show `ORG XX01 / WS XX03` scope. A workspace manager may assign only roles authorized by server policy and below their effective role ceiling. “Create account” and “Invite member” remain separate page flows.

## 6. Group and profile pages

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / ORG XX01 / WS XX03 / Group A                                  │
├────────────────────────────────────────────────────────────────────────────┤
│ Group A                         [Create profile] [Manage members]          │
│ Tabs: Profiles | Members & access | Audit                                  │
├────────────────────────────────────────────────────────────────────────────┤
│ Search profiles...   Runtime ▾   Sync ▾                   [Refresh]        │
│                                                                            │
│ Profile       Member             Runtime       Sync        Action           │
│ Profile 01    alice@…            Stopped       Synced      [Details →]      │
│ Profile 02    bob@…              Running       Uploading   [Details →]      │
│                                                                            │
│ Selected: [Move to group…] [Change access…] [Stop]                          │
└────────────────────────────────────────────────────────────────────────────┘
```

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / ORG XX01 / WS XX03 / Profile 01                               │
├────────────────────────────────────────────────────────────────────────────┤
│ Profile 01                       [Move to group…] [Manage access]           │
│ Tabs: Overview | Members & access | Sync history | Activity                 │
├────────────────────────────────────────────────────────────────────────────┤
│ Group: Group A   Runtime: Stopped   Sync: Synced                            │
│ Principal     Scope                Capabilities              Status         │
│ alice@…       Workspace member     Use, edit                 Active         │
│ bob@…         Profile grant        Use                       Active         │
│                                                                            │
│ [Grant access] [Revoke access] [Move profile]                               │
└────────────────────────────────────────────────────────────────────────────┘
```

## 7. Account creation and invitation are separate workflows

```text
CREATE ACCOUNT (direct provisioning; current endpoint is workspace-scoped)
Organizations → ORG XX01 → Workspaces → WS XX03 → Members → [Create account]
  → dedicated page shows breadcrumb, destination workspace, role selector
  → enter username/email and choose an API-grantable workspace role
  → API validates actor, role ceiling, workspace scope, uniqueness and input
  → create identity + active workspace membership; login may await email verification
  → return canonical account/member result; audit and refresh workspace members

INVITE MEMBER (acceptance required)
Organizations → ORG XX01 → Workspaces → WS XX03 → Members → [Invite]
  → separate invite page; enter email + allowed workspace role
  → API creates workspace-scoped pending invite and sends it
  → recipient accepts invitation
  → API activates workspace membership and records the action

EMAIL ALREADY EXISTS
  → do not create a duplicate identity
  → show only policy-authorized identity details
  → use invite/join or a separately authorized membership grant flow
```

Creating an account does not create an invitation. Creating an invitation does not imply membership exists before acceptance. The current direct-create endpoint creates at a workspace boundary; the UI must not promise org-wide account membership. Both actions require backend authorization and durable audit records.

## 7.1 Who can manage which lower-level accounts/resources

```text
PLATFORM ADMIN
  ├─ account directory: create / invite / role / suspend (platform policy)
  └─ any ORG → WS → GROUP or UNGROUPED → PROFILE

ORG OWNER (ORG XX01)
  ├─ choose one owned workspace before member/account action
  ├─ create account OR invite → role is below owner ceiling
  ├─ manage that workspace's membership status / role as API allows
  └─ manage descendant WS / GROUP / PROFILE only where API grants it

WS OWNER / ADMIN / LEAD
  ├─ create account OR invite only when capability + scope permit
  ├─ grant only a role below the actor's effective role ceiling
  └─ manage groups/profiles in this WS only with the required resource grant

MEMBER
  └─ use own membership and explicitly granted resources; no implied admin power

At every arrow, backend authorization is authoritative. A role name is not a grant.
```

## 8. Resource movement and ungrouped profiles

```text
Profile in a workspace
├── Move to another group in the same workspace
├── Move to Ungrouped (no group is valid)
└── Move to another workspace
    ├── verify source-scope management permission
    ├── verify destination-scope create/manage permission
    ├── inspect active browser process, lease, and pending sync
    ├── show destination and access changes before confirmation
    ├── perform one authorized, durable move operation
    └── write audit record and refresh both inventories
```

Cross-workspace or cross-organization moves must preserve or explicitly re-evaluate membership grants, ownership, quotas, active sessions, sync state, and audit history. If the API cannot guarantee these invariants, it must reject the move with a specific reason instead of partially moving the resource.

## 9. Sync and application update states

```text
PROFILE SYNC
Synced
├── last successful sync time and device
└── [Sync now]

Pending upload / download
├── current operation and progress when known
└── reason for waiting

Conflict / Failed
├── actionable error and support reference
├── safe retry or conflict-resolution action
└── preserve local and remote data until resolution
```

```text
APP UPDATE
Up to date → show installed version and last check time

Update available
├── show current → offered version, release notes, and verified package
├── [Download]
└── after download: [Install when app closes] [Later]

Installing
├── stop app and daemon safely
├── replace application binaries
├── restart and verify installed version
└── on failure, restore prior binaries and preserve user data
```

Checking for updates must not immediately install or close the application. Update and sync status must be derived from real backend/runtime state, not simulated UI progress.

## 10. Authorization and consistency rules

```text
Request = actor + action + target + scope

API authorization
├── resolve the actor's effective identity and grants
├── verify the actor can perform the action in the target scope
├── verify the requested role is grantable by this actor
├── validate parent/child relationships and destination scope
├── commit the mutation and audit event atomically where possible
└── return the canonical updated resource and membership state

UI
├── render actions from capabilities returned by the API
├── refresh affected lists/details after a successful mutation
├── preserve route, filters, and pagination where safe
└── display precise forbidden, conflict, offline, and validation states
```

UI visibility is not an authorization boundary. Every mutation must be enforced by the API. A successful mutation must update the affected UI state without requiring a page reload.

## 11. Implementation acceptance checklist

- Organization → Workspace → Group/Ungrouped → Profile routes are linkable and preserve scope.
- Platform Admin, Organization Admin, Workspace Admin, Lead, and Member receive API-derived capabilities.
- Direct account creation and member invitation have separate APIs and pages.
- Account creation assigns the new identity to the selected authorized organization/workspace.
- Member suspension is scoped and its impact on lower-level access is explicit.
- Managers can create only roles and accounts allowed by their effective grants.
- Group/profile inventories support search, filters, pagination, empty/loading/error/forbidden states, and refresh.
- Profile can remain ungrouped and can move only through an authorized, auditable operation.
- Sync and updater states show real status and actionable errors.
- UI uses approved Donut patterns and tokens per [UI authority](ui-authority.md); this ASCII is not a production UI reference.
- Screenshot, keyboard, responsive, API authorization, and end-to-end interaction evidence are recorded before release.

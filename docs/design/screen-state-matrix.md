# BugLogin 2.0 Per-Screen State Matrix

Status: required implementation contract  
Last updated: 2026-07-20  
Parent: [DESIGN.md](../../DESIGN.md)

## 1. State definitions

| Code | State | Required behavior |
|---|---|---|
| `L` | Loading | Preserve shell and known context; use skeleton only for predictable structure |
| `E0` | First-use empty | Explain object/job and provide permitted first action |
| `EF` | Filter/search empty | Preserve filters; show result count and clear/reset action |
| `ER` | Recoverable error | Name failed operation, retained data, retry path, correlation/details |
| `EU` | Unrecoverable error | Stop unsafe continuation; provide support/export/recovery path |
| `F` | Forbidden | Explain insufficient access without leaking protected data |
| `O` | Offline | Preserve local capability; mark remote-only actions unavailable with reason |
| `D` | Degraded/partial | Show available versus unavailable data/actions and freshness |
| `K` | Locked/leased | Identify lock owner/state, expiry, allowed safe alternatives |
| `S` | Success/ready | Real data and actions available; avoid redundant success decoration |
| `P` | In progress | Durable job/stage/progress; safe cancel/leave behavior |
| `X` | Conflict | Compare versions/scope; safe default; explicit resolution |
| `R` | Revoked/expired | Block protected action; show changed access/session/token state |
| `A` | Archived/deleted | Read-only evidence or recovery window where policy allows |

Legend: `●` required, `○` conditional, `—` not applicable.

## 2. State presentation rules

- Page-level state replaces only the affected content region, not global navigation.
- Component-level failures stay local unless authority or data integrity is uncertain.
- Mutations use pessimistic confirmation for permissions, destructive actions, lease changes, sync resolution, token creation, and migration.
- Optimistic updates are limited to reversible local preferences with reliable rollback.
- Stale cached data displays timestamp and source; never appears current without qualification.
- A hidden control is allowed only when exposing it would leak protected capability. Otherwise disable with reason.

## 3. Public and account matrices

### 3.1 Public web

| Screen group | L | E0 | EF | ER | EU | F | O | D | S |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| PUB-01–08 Marketing/product/security | ● | — | ○ | ● | ○ | — | ○ | ○ | ● |
| PUB-09 Download | ● | — | — | ● | ● | ○ | ○ | ● | ● |
| PUB-10 Release notes | ● | ○ | ○ | ● | ○ | — | ○ | ○ | ● |
| PUB-11 Docs landing | ● | ○ | ● | ● | ○ | — | ○ | ○ | ● |
| PUB-12 Status | ● | ○ | — | ● | ○ | — | ○ | ● | ● |
| PUB-13 Pricing/availability | ● | ○ | — | ● | ○ | ○ | ○ | ○ | ● |
| PUB-14 Legal | ● | — | — | ● | ● | ○ | ○ | ○ | ● |
| PUB-15 Route error | — | — | — | ● | ● | — | — | — | ○ |

Public-specific checks:

- Download errors preserve manual platform choices and checksum access.
- Status degradation names affected components and update time.
- Security content avoids unapproved claims and clearly dates assurance material.

### 3.2 Identity/account

| Screen | L | E0 | EF | ER | EU | F | O | D | K | S | P | R |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| ACC-01 Sign up | ○ | — | — | ● | ● | ○ | ● | ○ | ○ | ● | ● | ○ |
| ACC-02 Sign in | ○ | — | — | ● | ● | ○ | ● | ○ | ○ | ● | ● | ● |
| ACC-03 MFA | ○ | — | — | ● | ● | ○ | ● | ○ | ○ | ● | ● | ● |
| ACC-04 Recovery (outbound password reset unavailable this release) | ○ | — | — | ● | ● | ○ | ● | ○ | ○ | ● | ● | ● |
| ACC-05 Invitation | ● | — | — | ● | ● | ● | ● | ○ | — | ● | ● | ● |
| ACC-06 Onboarding | ● | ○ | — | ● | ● | ● | ● | ○ | — | ● | ● | ○ |
| ACC-07 Profile | ● | — | — | ● | ○ | ● | ○ | ○ | — | ● | ● | ○ |
| ACC-08 Security | ● | ○ | — | ● | ● | ● | ○ | ○ | ○ | ● | ● | ● |
| ACC-09 Sessions/devices | ● | ● | ○ | ● | ● | ● | ○ | ● | — | ● | ● | ● |
| ACC-10 API/MCP tokens | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ● | ● |
| ACC-11 Notifications | ● | ○ | — | ● | ○ | ● | ○ | ● | — | ● | ● | ○ |
| ACC-12 Connected apps | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ● | ● |
| ACC-13 Account deletion | ● | — | — | ● | ● | ● | ○ | ○ | ○ | ● | ● | ○ |

Account-specific checks:

- Token secret appears once after confirmed creation; later states show prefix and metadata only.
- Session revoke distinguishes current session, other sessions, all sessions, and failed partial revoke.
- Invitation state distinguishes valid, accepted, expired, revoked, wrong account, and workspace unavailable.

## 4. Workspace matrices

### 4.1 Inventories and details

| Screen group | L | E0 | EF | ER | EU | F | O | D | K | S | P | X | R | A |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| WSP-01 Overview | ● | ○ | ○ | ● | ● | ● | ○ | ● | ○ | ● | ○ | ○ | ○ | ○ |
| WSP-02 Profiles inventory | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| WSP-03 Profile detail | ● | — | — | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| WSP-04 Profile access | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ○ |
| WSP-05 Profile activity | ● | ● | ● | ● | ○ | ● | ○ | ● | — | ● | ○ | — | ○ | ○ |
| WSP-06–13 Resource screens | ● | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | ● | ● | ● | ● |
| WSP-14 Members | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ● | ○ | ● | ● |
| WSP-15 Member detail | ● | — | — | ● | ● | ● | ○ | ● | — | ● | ● | ○ | ● | ● |
| WSP-16 Teams | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ● | ○ | ○ | ● |
| WSP-17 Team detail | ● | ○ | ● | ● | ● | ● | ○ | ● | — | ● | ● | ● | ○ | ● |
| WSP-18 Grants | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| WSP-19 Roles/capabilities | ● | ○ | ● | ● | ● | ● | ○ | ● | — | ● | ● | ● | ○ | ● |
| WSP-20 Invitations | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ● | — | ● | ● |
| WSP-21 Audit | ● | ● | ● | ● | ● | ● | ○ | ● | — | ● | ○ | — | ○ | ● |
| WSP-22 Jobs | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |

### 4.2 Settings and control

| Screen | L | E0 | ER | EU | F | O | D | K | S | P | X | R | A |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| WSP-23 Usage/quotas | ● | ○ | ● | ○ | ● | ○ | ● | — | ● | ○ | ○ | ○ | ○ |
| WSP-24 Billing | ● | ○ | ● | ● | ● | ○ | ● | ○ | ● | ● | ○ | ○ | ● |
| WSP-25 General settings | ● | — | ● | ● | ● | ○ | ● | ○ | ● | ● | ● | ○ | ● |
| WSP-26 Policies | ● | ○ | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ○ | ● |
| WSP-27 Sync/storage | ● | ○ | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |
| WSP-28 Retention/backup | ● | ○ | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ○ | ● |
| WSP-29 Integrations | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | ● | ● | ● | ● |
| WSP-30 Danger zone | ● | — | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ○ | ● |
| WSP-31 Reconciliation | ● | ○ | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |

Workspace-specific checks:

- Effective-access views distinguish direct grants, team grants, inherited role, policy constraint, expiry, and revocation.
- Cross-workspace stale data shows source workspace and freshness.
- Bulk mutations report succeeded, failed, skipped, unchanged, and retryable subsets.
- Archived resources remain distinguishable from deleted or revoked resources.

## 5. Desktop matrices

### 5.1 Profiles and resources

| Screen group | L | E0 | EF | ER | EU | F | O | D | K | S | P | X | R | A |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| DES-01 Profiles home | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |
| DES-02 Profile detail | ● | — | — | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |
| DES-03 Create profile | ● | — | — | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | — |
| DES-04 Bulk create | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | — |
| DES-05 Running profiles | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ |
| DES-06 Shared with me | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |
| DES-07 Groups/saved views | ● | ● | ● | ● | ○ | ● | ● | ● | ○ | ● | ● | ● | ● | ● |
| DES-08–14 Resource screens | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |

### 5.2 Jobs, diagnostics, migration, settings

| Screen group | L | E0 | EF | ER | EU | F | O | D | K | S | P | X | R | A |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| DES-15 Jobs center | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● |
| DES-16–20 Diagnostics | ● | ○ | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ○ | ○ |
| DES-21 Migration inspect | ● | ● | — | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ○ |
| DES-22 Migration mapping | ● | ○ | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ○ |
| DES-23 Dry run/report | ● | ○ | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ● |
| DES-24 Execute/verify | ● | — | — | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ● |
| DES-25–30 Settings | ● | ○ | ○ | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | ○ |
| DES-31 First run | ● | ○ | — | ● | ● | ○ | ● | ● | ○ | ● | ● | ○ | ○ | — |
| DES-32 Offline/degraded center | ● | ○ | ○ | ● | ● | ○ | ● | ● | ○ | ● | ● | ● | ● | ○ |

Desktop-specific checks:

- Local launch remains available offline unless a shared lease or explicit policy requires authority.
- Runtime unavailable, runtime corrupt, update required, incompatible profile, proxy failure, and launch crash are distinct states.
- Closing the app while jobs run explains continuation or cancellation behavior.
- Sync state distinguishes local-only, queued, uploading, downloading, synced, paused, conflict, checksum failure, revoked, and remote unavailable.
- Migration source is never mutated; all screens reinforce inspect/snapshot/dry-run boundaries.

## 6. Platform admin matrix

| Screen group | L | E0 | EF | ER | EU | F | O | D | K | S | P | X | R | A |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| ADM-01 Overview | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ○ |
| ADM-02–06 Tenant/access | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-07 Jobs/queues | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-08 Storage/sync | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-09 Runtime/update | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-10 Feature access/quotas | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-11 Security/sessions | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-12 Audit/investigations | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-13–14 Support/impersonation | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-15 Incidents | ● | ● | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-16 Platform settings | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |
| ADM-17 Migration control | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ● | ● | ● | ● |

Admin-specific checks:

- Every privileged mutation includes actor, reason, scope, confirmation, result, audit ID, and recovery status.
- Impersonation persists a visually unmistakable banner across every screen.
- Break-glass and normal admin states are visually and semantically distinct.
- Partial cross-tenant operations never summarize as complete success.

## 7. Component state minimums

| Component | Required states |
|---|---|
| Button | default, hover, focus, active, disabled with reason, loading, success only when action persists |
| Input | empty, filled, hover, focus, invalid, valid where useful, disabled, read-only, loading lookup |
| Select/combobox | closed, open, loading options, no options, no match, invalid, disabled |
| Table | loading, populated, first-use empty, filtered empty, stale, partial rows, selection, bulk progress, error |
| Drawer/dialog | opening, ready, validation error, submitting, server error, success/close, unsaved dismissal |
| Toast/banner | info, success, warning, error, action available, dismissible/non-dismissible |
| Job | queued, running, pausing, paused, canceling, canceled, succeeded, partial, failed, retrying, expired |
| Profile | stopped, launching, running, parking, parked, stopping, crashed, incompatible, locked, revoked |
| Sync | local-only, queued, syncing, synced, paused, offline, conflict, checksum failure, revoked |
| Token/session | active, expiring, expired, revoked, compromised, last-used unknown |

## 8. State acceptance template

```text
Screen ID:
Role/capability:
Data volume: empty / typical / maximum practical
Network: online / offline / slow / partial
State code:
Primary action result:
Destructive action result:
Keyboard path:
Screen-reader announcement:
Viewport:
Screenshot artifact:
Known residual risk:
```

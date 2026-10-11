# BugLogin 2.0 Information Architecture and Screen Inventory

Status: implementation planning baseline  
Last updated: 2026-07-20  
Parent: [DESIGN.md](../../DESIGN.md)

## 1. Navigation model

```text
Public web
├── Home
├── Product
│   ├── Profiles and runtimes
│   ├── Proxy and network
│   ├── Sharing and control
│   ├── Sync and recovery
│   └── Automation/API/MCP
├── Security
├── Download
├── Docs
├── Status
└── Legal

Identity and account
├── Sign up
├── Sign in
├── MFA / recovery
├── Accept invite
├── Account profile
├── Security
├── Sessions and devices
├── API/MCP tokens
├── Notifications
└── Connected apps

Workspace web
├── Overview
├── Profiles
├── Resources
│   ├── Proxies
│   ├── VPN configurations
│   ├── Cookie sets
│   ├── Extensions
│   ├── Extension groups
│   └── Runtime entitlements
├── People
│   ├── Members
│   └── Teams
├── Access
│   ├── Grants
│   ├── Roles and capabilities
│   └── Invitations
├── Activity
│   ├── Audit
│   └── Jobs
├── Usage and billing
└── Settings
    ├── General
    ├── Policies
    ├── Sync and storage
    ├── Retention and backup
    ├── Integrations
    └── Danger zone

Desktop app
├── Profiles
│   ├── All profiles
│   ├── Running
│   ├── Shared with me
│   ├── Groups and saved views
│   ├── Profile detail
│   └── Create / bulk create
├── Resources
│   ├── Proxy library and parser
│   ├── VPN configurations
│   ├── Cookie sets
│   ├── Extensions and groups
│   └── Runtime catalog
├── Jobs
├── Diagnostics
│   ├── System health
│   ├── Runtime health
│   ├── Network tests
│   ├── Logs
│   └── Support bundle
├── Migration
└── Settings
    ├── General
    ├── Storage and sync
    ├── Runtime updates
    ├── Automation/API/MCP
    ├── Security
    └── About

Platform admin
├── Overview
├── Organizations
├── Workspaces
├── Members and identities
├── Resources and grants
├── Jobs and queues
├── Storage and sync health
├── Runtime/update catalog
├── Feature access and quotas
├── Security and sessions
├── Audit and investigations
├── Support tools
├── Incidents
└── Platform settings
```

## 2. Route principles

- Routes reflect stable domain nouns and IDs, not presentation layouts.
- Organization/workspace context is explicit in workspace routes.
- Detail routes remain linkable; drawers may mirror a route but cannot be the only access path.
- Filters, sort, pagination, selected tab, and saved view are URL-addressable on web.
- Desktop route state survives restart for the last safe screen, excluding transient secrets and destructive dialogs.
- Forbidden resources return a dedicated forbidden state without leaking existence beyond policy.
- Legacy redirects exist only for confirmed high-value links and never reproduce obsolete IA.

## 3. Full screen inventory

Priority: P0 launch-critical, P1 required operational breadth, P2 conditional/later.

### 3.1 Public web

| ID | Screen | Primary job | Priority | Primary action |
|---|---|---|---:|---|
| PUB-01 | Home | Understand product and choose next step | P0 | Download / create account |
| PUB-02 | Product overview | Evaluate product capabilities | P0 | Download |
| PUB-03 | Profiles and runtimes | Understand profile/runtime model | P1 | View supported runtimes |
| PUB-04 | Proxy and network | Understand supported network controls | P1 | Read documentation |
| PUB-05 | Sharing and control | Evaluate team governance | P1 | Create account |
| PUB-06 | Sync and recovery | Understand local-first sync/recovery | P1 | Read security model |
| PUB-07 | Automation/API/MCP | Evaluate automation surface | P1 | Open docs |
| PUB-08 | Security | Review trust, encryption, updates, reporting | P0 | Report issue / read details |
| PUB-09 | Download | Select verified build | P0 | Download build |
| PUB-10 | Release notes | Review changes and compatibility | P1 | Download release |
| PUB-11 | Docs landing | Find setup and reference docs | P0 | Open guide |
| PUB-12 | Status | Check service health/incidents | P1 | Subscribe |
| PUB-13 | Pricing/availability | Understand plans or self-host terms | P2 | Select plan/contact |
| PUB-14 | Legal index | Access terms/privacy/licenses | P0 | Open document |
| PUB-15 | 404/route error | Recover from invalid link | P0 | Search / return home |

### 3.2 Identity and account

| ID | Screen | Primary job | Priority | Primary action |
|---|---|---|---:|---|
| ACC-01 | Sign up | Create identity | P0 | Create account |
| ACC-02 | Sign in | Start secure session | P0 | Sign in |
| ACC-03 | MFA challenge | Prove second factor | P0 | Verify |
| ACC-04 | Recovery | Recover account | P0 | Continue recovery |
| ACC-05 | Accept invitation | Join organization/workspace knowingly | P0 | Accept invite |
| ACC-06 | Onboarding | Create/select organization and workspace | P0 | Continue |
| ACC-07 | Account profile | Manage identity basics | P1 | Save changes |
| ACC-08 | Security | Manage password/MFA/recovery | P0 | Update security |
| ACC-09 | Sessions and devices | Review/revoke sessions | P0 | Revoke session(s) |
| ACC-10 | API/MCP tokens | Create and revoke credentials | P0 | Create token |
| ACC-11 | Notifications | Manage delivery preferences | P1 | Save preferences |
| ACC-12 | Connected apps | Review/revoke integrations | P1 | Revoke connection |
| ACC-13 | Account deletion | Request destructive account action | P1 | Request deletion |

### 3.3 Workspace web

| ID | Screen | Primary job | Priority | Primary action |
|---|---|---|---:|---|
| WSP-01 | Workspace overview | Assess current operations and exceptions | P0 | Resolve highest-priority issue |
| WSP-02 | Profiles inventory | Find, inspect, govern profiles | P0 | Create/profile action |
| WSP-03 | Profile detail | Review state, access, runtime, activity | P0 | Contextual action |
| WSP-04 | Profile access | Manage grants and lease policy | P0 | Grant access |
| WSP-05 | Profile activity | Investigate profile events | P1 | Filter/export audit |
| WSP-06 | Resource inventory | Find all typed resources | P0 | Add resource |
| WSP-07 | Proxy inventory | Manage proxies at workspace scope | P0 | Import proxies |
| WSP-08 | Proxy import/parse | Validate and add proxy batch | P0 | Commit valid rows |
| WSP-09 | VPN configurations | Govern VPN resources | P1 | Add configuration |
| WSP-10 | Cookie sets | Govern reusable cookie sets | P1 | Add cookie set |
| WSP-11 | Extensions | Govern extension catalog | P1 | Add extension |
| WSP-12 | Extension groups | Manage approved bundles | P1 | Create group |
| WSP-13 | Runtime entitlements | Review runtime availability/policy | P1 | Assign entitlement |
| WSP-14 | Members | Manage workspace members | P0 | Invite member |
| WSP-15 | Member detail | Review access, sessions, activity | P0 | Change access |
| WSP-16 | Teams | Manage team groupings | P0 | Create team |
| WSP-17 | Team detail | Manage membership and grants | P0 | Add member/resource |
| WSP-18 | Grants | Audit and manage resource access | P0 | Create/revoke grant |
| WSP-19 | Roles/capabilities | Review role definitions | P1 | Edit role if custom roles approved |
| WSP-20 | Invitations | Track pending/expired invites | P0 | Invite/resend/revoke |
| WSP-21 | Audit | Investigate workspace actions | P0 | Filter/export |
| WSP-22 | Jobs | Track durable operations | P0 | Retry/cancel safe job |
| WSP-23 | Usage/quotas | Understand consumption and limits | P1 | Adjust/upgrade/request |
| WSP-24 | Billing | Manage billing records | P2 | Update billing |
| WSP-25 | General settings | Manage name, locale, defaults | P0 | Save settings |
| WSP-26 | Policies | Configure runtime/share/security policy | P0 | Save policy |
| WSP-27 | Sync/storage | Configure storage and inspect health | P0 | Test/save configuration |
| WSP-28 | Retention/backup | Configure retention and recovery | P1 | Save policy / restore |
| WSP-29 | Integrations | Manage API/webhook/external integrations | P1 | Add integration |
| WSP-30 | Danger zone | Transfer/archive/delete workspace | P1 | Initiate guarded action |
| WSP-31 | Reconciliation report | Resolve migration/account mappings | P0 | Confirm resolution |

### 3.4 Desktop app

| ID | Screen | Primary job | Priority | Primary action |
|---|---|---|---:|---|
| DES-01 | Profiles home | Launch and manage profiles quickly | P0 | Launch/create |
| DES-02 | Profile detail | Inspect/edit one profile | P0 | Launch/save |
| DES-03 | Create profile | Create compatible profile | P0 | Create / create and launch |
| DES-04 | Bulk create | Preview and create profile batch | P1 | Commit valid rows |
| DES-05 | Running profiles | Monitor and stop/park sessions | P0 | Open/stop/park |
| DES-06 | Shared with me | Use governed shared profiles | P0 | Request/launch/view |
| DES-07 | Groups/saved views | Organize repeated work | P1 | Create group/view |
| DES-08 | Proxy library | Find/test/assign proxies | P0 | Add/import/test |
| DES-09 | Proxy parser | Parse large mixed proxy input | P0 | Add valid proxies |
| DES-10 | VPN configurations | Manage local/assigned VPN configs | P1 | Add/test/assign |
| DES-11 | Cookie sets | Import/export/apply cookie data | P1 | Import/apply/export |
| DES-12 | Extensions | Install/assign approved extensions | P1 | Add/assign |
| DES-13 | Extension groups | Apply reusable extension bundles | P1 | Create/apply group |
| DES-14 | Runtime catalog | Install/update/verify runtimes | P0 | Install/update/verify |
| DES-15 | Jobs center | Monitor imports, sync, migration, backup | P0 | Inspect/retry/cancel |
| DES-16 | System health | Diagnose desktop dependencies | P0 | Run checks |
| DES-17 | Runtime health | Diagnose installed runtime | P0 | Verify/repair |
| DES-18 | Network tests | Diagnose proxy/VPN/DNS path | P0 | Run test |
| DES-19 | Logs | Inspect and export local logs | P1 | Filter/export |
| DES-20 | Support bundle | Create redacted support artifact | P1 | Review/create bundle |
| DES-21 | Migration inspect | Select and inspect source | P0 | Inspect |
| DES-22 | Migration mapping | Resolve entity/profile mapping | P0 | Confirm mappings |
| DES-23 | Migration dry run/report | Prove result before writes | P0 | Run/review |
| DES-24 | Migration execute/verify | Execute and validate migration | P0 | Execute/verify |
| DES-25 | General settings | Configure startup/locale/defaults | P0 | Save |
| DES-26 | Storage/sync settings | Configure local storage and sync | P0 | Test/save |
| DES-27 | Runtime updates | Configure update channel/policy | P0 | Check/save |
| DES-28 | Automation/API/MCP | Manage local automation access | P0 | Enable/create/revoke |
| DES-29 | Security | Manage local lock/secrets/session | P0 | Save/revoke |
| DES-30 | About/update | Inspect version/license/update integrity | P0 | Check update |
| DES-31 | First run | Choose data path, account, workspace | P0 | Continue |
| DES-32 | Offline/degraded center | Understand unavailable remote functions | P0 | Retry/work locally |

### 3.5 Platform admin

| ID | Screen | Primary job | Priority | Primary action |
|---|---|---|---:|---|
| ADM-01 | Platform overview | Assess service and security health | P0 | Open incident/queue |
| ADM-02 | Organizations | Find and govern organizations | P0 | Inspect organization |
| ADM-03 | Organization detail | Review status, plan, policy, incidents | P0 | Guarded admin action |
| ADM-04 | Workspaces | Find and govern workspaces | P0 | Inspect workspace |
| ADM-05 | Identities/members | Investigate identity and access | P0 | Revoke/reset/inspect |
| ADM-06 | Resources/grants | Investigate access graph | P0 | Revoke/repair grant |
| ADM-07 | Jobs/queues | Diagnose platform background work | P0 | Retry/cancel/quarantine |
| ADM-08 | Storage/sync health | Diagnose manifests, chunks, conflicts | P0 | Repair/quarantine |
| ADM-09 | Runtime/update catalog | Publish and govern signed artifacts | P0 | Stage/publish/revoke |
| ADM-10 | Feature access/quotas | Manage entitlements and limits | P1 | Adjust with reason |
| ADM-11 | Security/sessions | Investigate sessions/tokens/events | P0 | Revoke/contain |
| ADM-12 | Audit/investigations | Correlate cross-domain events | P0 | Save/export case |
| ADM-13 | Support tools | Assist user with minimum privilege | P0 | Start guarded support action |
| ADM-14 | Impersonation session | Reproduce user-visible issue | P1 | Exit impersonation |
| ADM-15 | Incidents | Coordinate operational incident | P0 | Update incident |
| ADM-16 | Platform settings | Manage global policy/integrations | P0 | Save guarded change |
| ADM-17 | Migration control | Monitor cutover and rollback evidence | P0 | Pause/continue/rollback |

## 4. Cross-surface ownership

| Domain | Desktop | Workspace web | Account | Platform admin |
|---|---|---|---|---|
| Profile launch/runtime | Primary | Govern/inspect | None | Diagnose only |
| Resource configuration | Local use + assignment | Authority and policy | None | Investigate/repair |
| Member/team/access | Consume effective access | Primary authority | Personal identity only | Cross-tenant support/security |
| Sessions/tokens | Local automation token use | Workspace integration tokens | Personal sessions/tokens | Emergency revoke/investigate |
| Sync/storage | Local state and transfer | Policy/health | None | Service health/repair |
| Migration | Local source and verification | Reconciliation/report | Re-auth if needed | Cutover/rollback oversight |
| Billing | None | Workspace/org management | Personal invoices if applicable | Support/admin |

## 5. High-risk reference flows

Prototype and approve before broad UI implementation:

1. Create profile, assign proxy, validate runtime compatibility, launch.
2. Share profile with capabilities and expiry; encounter lease; revoke access.
3. Import a large mixed proxy batch with partial failures and duplicates.
4. Diagnose sync interruption, resume, then resolve a conflict without data loss.
5. Invite member, add to team, grant resource, verify effective access, remove member.
6. Inspect legacy source, dry-run migration, resolve unsupported profiles, verify rollback evidence.
7. Create API/MCP token with scopes, use it, inspect audit, revoke it.
8. Platform support investigation with reason, temporary impersonation, persistent audit.

## 6. Inventory acceptance

A screen is ready for visual implementation only when:

- Primary user, job, authority, and data source are named.
- Primary and destructive actions map to backend capabilities.
- Route and return behavior are specified.
- Required states are marked in the state matrix.
- Responsive priority is defined.
- Screenshot viewports are assigned.
- No placeholder workflow or dead primary action remains.

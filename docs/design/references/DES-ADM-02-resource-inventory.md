# DES-ADM-02 Resource inventory

Status: implementation reference approved by the product owner on 2026-10-23 (ASCII direction).

## Screen contract

- Route: Workspace administration → Resources.
- Roles: any authenticated principal with server grants for `CAP-RES-001` read in the selected workspace.
- Primary job: find and inspect a resource and its direct grants without loading the entire workspace inventory.
- Data: cursor-paginated API resources, with detail and grant information read from their scoped APIs.
- Identity labels: show a verified member identity only when an authorized identity projection exists; otherwise use a localized generic label. Never show IDs as normal labels.

## Composition

```text
Resources  [Workspace v]
Search resources ...  [Type v] [State v]  Refresh
Selected: n profiles on this page  [Target group v]  [Assign group]
-----------------------------------------------------
Resource name | Type | State | Access summary
... one API page only ...
-----------------------------------------------------
Previous  Page n  Next

Selecting a row opens a detail region:
Overview: type, state, workspace
Access: direct grants (action/effect), source and effective summary
```

- Use the existing Donut-compatible shell, compact controls, semantic tokens and table patterns.
- Search and type/state filters are applied by the API before cursor pagination; the UI resets pagination when any filter changes.
- Selection is limited to profile rows on the loaded cursor page. Bulk group replacement previews the count and target, requires confirmation, and reports succeeded/failed profiles individually. Failed profiles remain selected for retry; it never implies selection of all matching results.
- Group replacement changes the complete group set for each selected profile. `No Group` is a valid target and clears all group memberships.
- Next/previous follows opaque server cursors. Never fetch all pages to paint the initial screen.
- A cross-workspace move or resource delete is not implied by this inventory screen.

## States

- Loading retains the shell and selected workspace.
- Empty workspace and filtered-empty have distinct messages.
- Error retains stale page rows with a retry action and visible stale status.
- Forbidden/offline errors are shown as API errors; no synthetic resource or grant state is shown.
- Resource detail and grants load independently from the page; failure in detail does not clear the inventory.

## Acceptance

- Initial load requests one bounded page; pagination uses returned cursors and guards cursor cycles.
- Changing workspace invalidates pending requests and resets cursor history.
- Detail shows only authoritative API fields. Grants are identified as direct grants; effective inherited/team access remains unavailable until an API exposes provenance.
- Keyboard operation and focus remain within existing desktop patterns.

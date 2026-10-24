# Organization and Workspace Management Plan

Status: in progress

## Phase 1 — Contract and UI audit

- [x] Read ownership and Donut UI authority.
- [x] Inspect existing organization/workspace/member/account API paths.
- [x] Confirm route and action permissions against actual server contracts: organization ownership is represented separately from workspace membership; account provisioning is a workspace-scoped endpoint.

## Phase 2 — Native hierarchy

- [x] Make the organization list the entry point and preserve selected organization.
- [ ] Make workspace selection drill into a dedicated scoped workspace page with breadcrumb and deep links.
- [x] Surface direct single/bulk account provisioning at workspace scope through a dedicated page; keep invitation as its separate existing flow.
- [x] Add a workspace action that switches the active scope before opening the profile inventory; groups remain reachable from the desktop navigation under that active workspace.
- [x] Convert workspace member management to a scoped page. Invitation creation remains a separate dialog flow.
- [x] Convert single invitation creation to a dedicated workspace page; bulk invitation still uses the existing dialog.
- [ ] Add group/profile deep links and selection state under the organization/workspace breadcrumb; workspace Group/Profile inventories are now reachable as dedicated tabs.

## Phase 3 — Verification

- [x] Run desktop package typecheck and focused self-tests; root application typecheck still reports 35 pre-existing errors in unrelated files and none in changed files.
- [x] Review design and diff for scope drift; document that membership is workspace-scoped and organization summaries cannot mutate a nonexistent org membership.
- [ ] Record interaction/visual evidence; runtime screenshot/interaction QA remains outstanding.

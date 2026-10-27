# Management UI rebuild execution

Spec: [canonical management design](../specs/2026-10-26-management-ui-ux-design.md).

User authorization: implement full production management flows from the approved ASCII; write new UI, keep Donut primitives and verified backend contracts. Canonical checkout/main only.

## Owned scope

- Single canonical checkout: `F:\Herd\buglogin`, branch `main`; all implementation changes are reviewed and integrated in this source.

## Execution

- [x] Read spec, UI authority, skills, current bridge/model and pinned Donut dialog baseline.
- [x] Establish nonoverlapping new-source ownership with existing authorized delegation.
- [x] Reconcile actual Org membership contract with design notes and document parent access gating.
- [x] Complete backend account single/bulk identity, generation and self-service slice.
- [x] Implement new Org/Workspace/Platform pages and real actions.
- [x] Implement new account/membership/self-service UI, including Org member suspend/reactivate bulk and single actions.
- [x] Implement new group/profile governance UI.
- [x] Replace production navigation bindings and disconnect retired management screens.
- [ ] Focused tests/typecheck/build, role/API and interaction/screenshot QA (automated contract/typecheck/build gates pass; visual QA is still required).
- [x] Record automated evidence and remaining limitations; retired screens were removed after reference checks.

Current state: implementation and automated integration gates complete in the canonical checkout. Visual interaction/screenshot QA and platform release evidence remain open; no release is claimed.
